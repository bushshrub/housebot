package registry

import (
	"sort"
	"testing"
	"time"
)

func mustMatch(t *testing.T, tags []string, track string) []Version {
	t.Helper()
	versions, err := matching(tags, track)
	if err != nil {
		t.Fatalf("matching(%v, %q): %v", tags, track, err)
	}
	return versions
}

func TestMatchingKeepsOnlySemverTagsOnTheTrack(t *testing.T) {
	tags := []string{
		"0.1.0-sha.8cf170e",
		"0.1.0-sha.4b21f0a",
		"0.2.0",
		"latest",
		"0.1.0-sha.notsemver!",
	}

	got := mustMatch(t, tags, "0.1.0-sha.*")

	if len(got) != 2 {
		t.Fatalf("expected 2 matches, got %d: %v", len(got), got)
	}
	for _, v := range got {
		if v.Tag != "0.1.0-sha.8cf170e" && v.Tag != "0.1.0-sha.4b21f0a" {
			t.Errorf("unexpected match %q", v.Tag)
		}
	}
}

// The tag `latest` matches a `*` track but is not a version, so it must not be
// resolvable — deploying it would defeat the point of pinning.
func TestMatchingDropsNonVersionTags(t *testing.T) {
	got := mustMatch(t, []string{"latest", "main", "0.1.0"}, "*")

	if len(got) != 1 || got[0].Tag != "0.1.0" {
		t.Fatalf("expected only 0.1.0, got %v", got)
	}
}

func TestHighestCoreIgnoresPrereleaseWhenComparingCores(t *testing.T) {
	versions := mustMatch(t, []string{"0.1.0-sha.aaa", "0.2.0-sha.bbb", "0.2.0-sha.ccc"}, "*")

	top := highestCore(versions)

	if len(top) != 2 {
		t.Fatalf("expected the two 0.2.0 candidates, got %v", top)
	}
	for _, v := range top {
		if v.semver.Minor() != 2 {
			t.Errorf("unexpected candidate %q", v.Tag)
		}
	}
}

// This is the whole reason creation timestamps are fetched: semver orders these
// two by the lexical order of a git hash, which carries no information.
func TestCreationTimeDecidesBetweenShaPrereleases(t *testing.T) {
	versions := mustMatch(t, []string{"0.1.0-sha.8cf170e", "0.1.0-sha.4b21f0a"}, "0.1.0-sha.*")

	older := time.Date(2026, 8, 28, 19, 2, 11, 0, time.UTC)
	for i := range versions {
		if versions[i].Tag == "0.1.0-sha.4b21f0a" {
			versions[i].Created = older.Add(time.Hour)
		} else {
			versions[i].Created = older
		}
	}

	sort.Slice(versions, func(i, j int) bool { return less(versions[j], versions[i]) })

	if versions[0].Tag != "0.1.0-sha.4b21f0a" {
		t.Fatalf("expected the later push to win, got %q", versions[0].Tag)
	}
	// Semver alone would have picked 8cf170e, since "8" sorts after "4". If the
	// ordering ever reverts to it, this test is what catches it.
	if !versions[0].semver.LessThan(versions[1].semver) {
		t.Fatal("expected the winner to be the semver-lesser tag, so the timestamp is what decided")
	}
}

func TestReleaseOutranksItsOwnPrerelease(t *testing.T) {
	versions := mustMatch(t, []string{"0.1.0", "0.1.0-sha.8cf170e"}, "*")

	// The prerelease is pushed later, which must not let it win: a real release
	// of the same core version is always the more finished artifact.
	for i := range versions {
		if versions[i].semver.Prerelease() != "" {
			versions[i].Created = time.Now()
		}
	}

	sort.Slice(versions, func(i, j int) bool { return less(versions[j], versions[i]) })

	if versions[0].Tag != "0.1.0" {
		t.Fatalf("expected the release to win, got %q", versions[0].Tag)
	}
}

func TestOrderingIsStableWithoutTimestamps(t *testing.T) {
	versions := mustMatch(t, []string{"0.1.0-sha.aaa", "0.1.0-sha.bbb"}, "*")

	sort.Slice(versions, func(i, j int) bool { return less(versions[j], versions[i]) })
	first := versions[0].Tag

	sort.Slice(versions, func(i, j int) bool { return less(versions[j], versions[i]) })

	if versions[0].Tag != first {
		t.Fatalf("ordering is not stable: %q then %q", first, versions[0].Tag)
	}
}

func TestMatchingRejectsAnInvalidTrackPattern(t *testing.T) {
	if _, err := matching([]string{"0.1.0"}, "[bad"); err == nil {
		t.Fatal("expected an invalid pattern to be an error")
	}
}
