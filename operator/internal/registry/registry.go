// Package registry resolves a track glob to a concrete chart version by
// listing tags in an OCI registry.
package registry

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"path"
	"sort"
	"time"

	"github.com/Masterminds/semver/v3"
	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	"github.com/google/go-containerregistry/pkg/v1/remote"
	"github.com/google/go-containerregistry/pkg/v1/remote/transport"
)

// createdAnnotation is what Helm stamps on a chart manifest at push time. It is
// the only ordering signal available for sha-suffixed prereleases, whose semver
// ordering is the lexical order of a git hash and therefore meaningless.
const createdAnnotation = "org.opencontainers.image.created"

// Client lists chart versions in a registry.
type Client struct {
	Keychain authn.Keychain

	// Insecure allows plain HTTP, for a registry inside the cluster that serves
	// no TLS. Off by default: GHCR and every other public registry is HTTPS.
	Insecure bool
}

// New returns a Client authenticating from the ambient Docker config, which in
// cluster is the image pull secret projected into the operator's Pod.
func New(insecure bool) *Client {
	return &Client{Keychain: authn.DefaultKeychain, Insecure: insecure}
}

func (c *Client) nameOptions() []name.Option {
	if c.Insecure {
		return []name.Option{name.Insecure}
	}
	return nil
}

// Version is one chart version found in the registry.
type Version struct {
	Tag     string
	Created time.Time

	semver *semver.Version
}

// Resolve returns the newest version in repo whose tag matches track.
func (c *Client) Resolve(ctx context.Context, repo, track string) (string, error) {
	if track == "" {
		return "", fmt.Errorf("no track to resolve")
	}

	repository, err := name.NewRepository(repo, c.nameOptions()...)
	if err != nil {
		return "", fmt.Errorf("parse repository %q: %w", repo, err)
	}

	tags, err := remote.List(repository,
		remote.WithContext(ctx),
		remote.WithAuthFromKeychain(c.Keychain),
	)
	if err != nil {
		return "", fmt.Errorf("list tags of %q: %w", repo, err)
	}

	candidates, err := matching(tags, track)
	if err != nil {
		return "", err
	}
	if len(candidates) == 0 {
		return "", fmt.Errorf("no tag in %q matches track %q", repo, track)
	}

	// Only the highest core version can win, so creation timestamps — a manifest
	// fetch each — are worth paying for on that group alone.
	top := highestCore(candidates)
	if len(top) > 1 {
		for i := range top {
			created, err := c.created(ctx, repository, top[i].Tag)
			if err != nil {
				return "", err
			}
			top[i].Created = created
		}
	}

	sort.Slice(top, func(i, j int) bool { return less(top[j], top[i]) })
	return top[0].Tag, nil
}

// Exists reports whether an exact tag is present, so that a pin can be rejected
// before it is written to the OCIRepository rather than after Flux fails on it.
func (c *Client) Exists(ctx context.Context, repo, tag string) (bool, error) {
	ref, err := name.NewTag(fmt.Sprintf("%s:%s", repo, tag), c.nameOptions()...)
	if err != nil {
		return false, fmt.Errorf("parse reference %s:%s: %w", repo, tag, err)
	}

	_, err = remote.Head(ref,
		remote.WithContext(ctx),
		remote.WithAuthFromKeychain(c.Keychain),
	)
	if err != nil {
		var status *transport.Error
		if errors.As(err, &status) &&
			(status.StatusCode == http.StatusNotFound || status.StatusCode == http.StatusForbidden) {
			return false, nil
		}
		return false, fmt.Errorf("head %s:%s: %w", repo, tag, err)
	}
	return true, nil
}

func (c *Client) created(ctx context.Context, repo name.Repository, tag string) (time.Time, error) {
	desc, err := remote.Get(repo.Tag(tag),
		remote.WithContext(ctx),
		remote.WithAuthFromKeychain(c.Keychain),
	)
	if err != nil {
		return time.Time{}, fmt.Errorf("fetch manifest %s:%s: %w", repo, tag, err)
	}

	var manifest struct {
		Annotations map[string]string `json:"annotations"`
	}
	if err := json.Unmarshal(desc.Manifest, &manifest); err != nil {
		return time.Time{}, fmt.Errorf("parse manifest %s:%s: %w", repo, tag, err)
	}

	raw, ok := manifest.Annotations[createdAnnotation]
	if !ok {
		// Ordering falls back to semver, which for prereleases is arbitrary but
		// stable — better than failing the whole poll.
		return time.Time{}, nil
	}

	created, err := time.Parse(time.RFC3339, raw)
	if err != nil {
		return time.Time{}, nil
	}
	return created, nil
}

// matching keeps the tags that satisfy the track glob and parse as semver. A
// tag that matches but is not a version cannot be a chart version, so it is
// dropped rather than treated as an error.
func matching(tags []string, track string) ([]Version, error) {
	if _, err := path.Match(track, ""); err != nil {
		return nil, fmt.Errorf("invalid track pattern %q: %w", track, err)
	}

	var out []Version
	for _, tag := range tags {
		ok, err := path.Match(track, tag)
		if err != nil || !ok {
			continue
		}
		parsed, err := semver.NewVersion(tag)
		if err != nil {
			continue
		}
		out = append(out, Version{Tag: tag, semver: parsed})
	}
	return out, nil
}

func highestCore(versions []Version) []Version {
	best := versions[0].semver
	for _, v := range versions[1:] {
		if coreLess(best, v.semver) {
			best = v.semver
		}
	}

	var out []Version
	for _, v := range versions {
		if !coreLess(v.semver, best) && !coreLess(best, v.semver) {
			out = append(out, v)
		}
	}
	return out
}

func coreLess(a, b *semver.Version) bool {
	if a.Major() != b.Major() {
		return a.Major() < b.Major()
	}
	if a.Minor() != b.Minor() {
		return a.Minor() < b.Minor()
	}
	return a.Patch() < b.Patch()
}

// less orders two versions of the same core version: a real release outranks
// any prerelease of it, and prereleases are ordered by push time.
func less(a, b Version) bool {
	aPre, bPre := a.semver.Prerelease() != "", b.semver.Prerelease() != ""
	if aPre != bPre {
		return aPre
	}
	if !a.Created.Equal(b.Created) {
		return a.Created.Before(b.Created)
	}
	return a.semver.LessThan(b.semver)
}
