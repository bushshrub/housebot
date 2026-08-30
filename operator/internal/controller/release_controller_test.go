package controller

import (
	"context"
	"fmt"
	"testing"

	helmv2 "github.com/fluxcd/helm-controller/api/v2"
	sourcev1 "github.com/fluxcd/source-controller/api/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	utilruntime "k8s.io/apimachinery/pkg/util/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	housebotv1alpha1 "github.com/bushshrub/housebot/operator/api/v1alpha1"
)

const (
	testNamespace = "housebot"
	testChartRepo = "ghcr.io/bushshrub/housebot/charts/housebot"
)

// stubRegistry answers from a fixed set of tags, with newest first.
type stubRegistry struct {
	tags     []string
	resolved int
}

func (s *stubRegistry) Resolve(_ context.Context, _, _ string) (string, error) {
	s.resolved++
	if len(s.tags) == 0 {
		return "", fmt.Errorf("no tags")
	}
	return s.tags[0], nil
}

func (s *stubRegistry) Exists(_ context.Context, _, tag string) (bool, error) {
	for _, candidate := range s.tags {
		if candidate == tag {
			return true, nil
		}
	}
	return false, nil
}

func testScheme(t *testing.T) *runtime.Scheme {
	t.Helper()
	scheme := runtime.NewScheme()
	utilruntime.Must(housebotv1alpha1.AddToScheme(scheme))
	utilruntime.Must(sourcev1.AddToScheme(scheme))
	utilruntime.Must(helmv2.AddToScheme(scheme))
	return scheme
}

func newRelease(spec housebotv1alpha1.ReleaseSpec) *housebotv1alpha1.Release {
	if spec.OCIRepositoryRef.Name == "" {
		spec.OCIRepositoryRef.Name = "housebot"
	}
	return &housebotv1alpha1.Release{
		ObjectMeta: metav1.ObjectMeta{Name: "housebot", Namespace: testNamespace},
		Spec:       spec,
	}
}

func newOCIRepository(tag string) *sourcev1.OCIRepository {
	repo := &sourcev1.OCIRepository{
		ObjectMeta: metav1.ObjectMeta{Name: "housebot", Namespace: testNamespace},
		Spec:       sourcev1.OCIRepositorySpec{URL: "oci://" + testChartRepo},
	}
	if tag != "" {
		repo.Spec.Reference = &sourcev1.OCIRepositoryRef{Tag: tag}
	}
	return repo
}

type harness struct {
	reconciler *ReleaseReconciler
	client     client.Client
	registry   *stubRegistry
}

func newHarness(t *testing.T, registry *stubRegistry, objects ...client.Object) *harness {
	t.Helper()
	scheme := testScheme(t)
	builder := fake.NewClientBuilder().
		WithScheme(scheme).
		WithStatusSubresource(&housebotv1alpha1.Release{})
	for _, object := range objects {
		builder = builder.WithObjects(object)
	}
	c := builder.Build()

	return &harness{
		reconciler: &ReleaseReconciler{Client: c, Scheme: scheme, Registry: registry},
		client:     c,
		registry:   registry,
	}
}

func (h *harness) reconcile(t *testing.T) ctrl.Result {
	t.Helper()
	result, err := h.reconciler.Reconcile(context.Background(), ctrl.Request{
		NamespacedName: types.NamespacedName{Name: "housebot", Namespace: testNamespace},
	})
	if err != nil {
		t.Fatalf("reconcile: %v", err)
	}
	return result
}

func (h *harness) release(t *testing.T) *housebotv1alpha1.Release {
	t.Helper()
	var release housebotv1alpha1.Release
	key := types.NamespacedName{Name: "housebot", Namespace: testNamespace}
	if err := h.client.Get(context.Background(), key, &release); err != nil {
		t.Fatalf("get Release: %v", err)
	}
	return &release
}

func (h *harness) ociTag(t *testing.T) string {
	t.Helper()
	var repo sourcev1.OCIRepository
	key := types.NamespacedName{Name: "housebot", Namespace: testNamespace}
	if err := h.client.Get(context.Background(), key, &repo); err != nil {
		t.Fatalf("get OCIRepository: %v", err)
	}
	return tagOf(&repo)
}

func TestTrackResolvesAndPatchesTheOCIRepository(t *testing.T) {
	h := newHarness(t,
		&stubRegistry{tags: []string{"0.1.0-sha.8cf170e"}},
		newRelease(housebotv1alpha1.ReleaseSpec{Track: "0.1.0-sha.*"}),
		newOCIRepository(""),
	)

	h.reconcile(t)

	if got := h.ociTag(t); got != "0.1.0-sha.8cf170e" {
		t.Fatalf("OCIRepository tag = %q", got)
	}

	release := h.release(t)
	if release.Status.Current != "0.1.0-sha.8cf170e" {
		t.Fatalf("status.current = %q", release.Status.Current)
	}
	if release.Status.Pinned {
		t.Fatal("a track-following Release must not be pinned")
	}
	if len(release.Status.History) != 1 || release.Status.History[0].By != housebotv1alpha1.SourceAuto {
		t.Fatalf("history = %#v", release.Status.History)
	}
}

// The interlock the whole design rests on: while spec.version is set, a newer
// chart sitting in the registry must not be deployed. Without this a rollback
// undoes itself on the next poll.
func TestPinSuppressesTheTrack(t *testing.T) {
	registry := &stubRegistry{tags: []string{"0.1.0-sha.broken", "0.1.0-sha.good"}}
	h := newHarness(t, registry,
		newRelease(housebotv1alpha1.ReleaseSpec{
			Track:   "0.1.0-sha.*",
			Version: "0.1.0-sha.good",
		}),
		newOCIRepository("0.1.0-sha.broken"),
	)

	h.reconcile(t)

	if got := h.ociTag(t); got != "0.1.0-sha.good" {
		t.Fatalf("expected the pin to win, OCIRepository tag = %q", got)
	}
	if registry.resolved != 0 {
		t.Fatal("the track must not be resolved at all while pinned")
	}

	release := h.release(t)
	if !release.Status.Pinned {
		t.Fatal("status.pinned must be true so alerts can surface it")
	}
	if release.Status.History[0].By != housebotv1alpha1.SourcePin {
		t.Fatalf("history should record the pin, got %q", release.Status.History[0].By)
	}
}

// Repeated polls while pinned must keep choosing the pin, not drift back to the
// track after the first reconcile.
func TestPinHoldsAcrossPolls(t *testing.T) {
	registry := &stubRegistry{tags: []string{"0.1.0-sha.broken", "0.1.0-sha.good"}}
	h := newHarness(t, registry,
		newRelease(housebotv1alpha1.ReleaseSpec{Track: "0.1.0-sha.*", Version: "0.1.0-sha.good"}),
		newOCIRepository("0.1.0-sha.broken"),
	)

	for i := 0; i < 3; i++ {
		h.reconcile(t)
		if got := h.ociTag(t); got != "0.1.0-sha.good" {
			t.Fatalf("poll %d drifted to %q", i, got)
		}
	}

	if history := h.release(t).Status.History; len(history) != 1 {
		t.Fatalf("an unchanged version must not append history, got %d entries", len(history))
	}
}

// Clearing spec.version is /resume: the Release rejoins its track.
func TestClearingTheVersionResumesTheTrack(t *testing.T) {
	registry := &stubRegistry{tags: []string{"0.1.0-sha.newest"}}
	h := newHarness(t, registry,
		newRelease(housebotv1alpha1.ReleaseSpec{Track: "0.1.0-sha.*", Version: "0.1.0-sha.old"}),
		newOCIRepository(""),
	)
	registry.tags = append(registry.tags, "0.1.0-sha.old")

	h.reconcile(t)
	if got := h.ociTag(t); got != "0.1.0-sha.old" {
		t.Fatalf("expected the pin first, got %q", got)
	}

	release := h.release(t)
	release.Spec.Version = ""
	if err := h.client.Update(context.Background(), release); err != nil {
		t.Fatalf("clear spec.version: %v", err)
	}

	h.reconcile(t)

	if got := h.ociTag(t); got != "0.1.0-sha.newest" {
		t.Fatalf("expected the track to resume, got %q", got)
	}
	if h.release(t).Status.Pinned {
		t.Fatal("status.pinned must clear when the pin is released")
	}
}

// A rollback target is read out of history, so history has to accumulate in the
// order the versions were deployed.
func TestHistoryAccumulatesNewestFirst(t *testing.T) {
	registry := &stubRegistry{tags: []string{"0.1.0-sha.one"}}
	h := newHarness(t, registry,
		newRelease(housebotv1alpha1.ReleaseSpec{Track: "0.1.0-sha.*"}),
		newOCIRepository(""),
	)

	h.reconcile(t)
	registry.tags = []string{"0.1.0-sha.two"}
	h.reconcile(t)

	history := h.release(t).Status.History
	if len(history) != 2 {
		t.Fatalf("expected 2 entries, got %#v", history)
	}
	if history[0].Version != "0.1.0-sha.two" || history[1].Version != "0.1.0-sha.one" {
		t.Fatalf("history is not newest-first: %#v", history)
	}
}

func TestHistoryIsBounded(t *testing.T) {
	limit := int32(3)
	registry := &stubRegistry{tags: []string{"0.1.0-sha.a"}}
	h := newHarness(t, registry,
		newRelease(housebotv1alpha1.ReleaseSpec{Track: "0.1.0-sha.*", HistoryLimit: &limit}),
		newOCIRepository(""),
	)

	for _, tag := range []string{"0.1.0-sha.a", "0.1.0-sha.b", "0.1.0-sha.c", "0.1.0-sha.d"} {
		registry.tags = []string{tag}
		h.reconcile(t)
	}

	history := h.release(t).Status.History
	if len(history) != 3 {
		t.Fatalf("expected history bounded to 3, got %d", len(history))
	}
	if history[0].Version != "0.1.0-sha.d" {
		t.Fatalf("expected the newest to survive, got %q", history[0].Version)
	}
}

// A pin naming a version that was never published must be refused before it
// reaches the OCIRepository, where it would only surface as a Flux pull failure.
func TestUnknownPinIsRefusedWithoutTouchingTheSource(t *testing.T) {
	h := newHarness(t,
		&stubRegistry{tags: []string{"0.1.0-sha.real"}},
		newRelease(housebotv1alpha1.ReleaseSpec{Track: "0.1.0-sha.*", Version: "0.1.0-sha.typo"}),
		newOCIRepository("0.1.0-sha.real"),
	)

	h.reconcile(t)

	if got := h.ociTag(t); got != "0.1.0-sha.real" {
		t.Fatalf("a bad pin must leave the source alone, got %q", got)
	}

	release := h.release(t)
	ready := conditionOf(release, housebotv1alpha1.ConditionReady)
	if ready == nil || ready.Status != metav1.ConditionFalse {
		t.Fatalf("expected Ready=False, got %#v", ready)
	}
}

// A digest on the same OCIRepository outranks the tag in Flux, so leaving one in
// place would let the Release report a version it had not actually deployed.
func TestPatchClearsADigestThatWouldOutrankTheTag(t *testing.T) {
	repo := newOCIRepository("")
	repo.Spec.Reference = &sourcev1.OCIRepositoryRef{
		Digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000",
		SemVer: ">=0.1.0",
	}

	h := newHarness(t,
		&stubRegistry{tags: []string{"0.1.0-sha.8cf170e"}},
		newRelease(housebotv1alpha1.ReleaseSpec{Track: "0.1.0-sha.*"}),
		repo,
	)

	h.reconcile(t)

	var updated sourcev1.OCIRepository
	key := types.NamespacedName{Name: "housebot", Namespace: testNamespace}
	if err := h.client.Get(context.Background(), key, &updated); err != nil {
		t.Fatalf("get OCIRepository: %v", err)
	}
	if updated.Spec.Reference.Digest != "" || updated.Spec.Reference.SemVer != "" {
		t.Fatalf("digest and semver must be cleared, got %#v", updated.Spec.Reference)
	}
	if updated.Spec.Reference.Tag != "0.1.0-sha.8cf170e" {
		t.Fatalf("tag = %q", updated.Spec.Reference.Tag)
	}
}

func TestHelmReleaseReadinessIsMirrored(t *testing.T) {
	hr := &helmv2.HelmRelease{
		ObjectMeta: metav1.ObjectMeta{Name: "housebot", Namespace: testNamespace},
		Status: helmv2.HelmReleaseStatus{
			LastAttemptedRevision: "0.1.0-sha.8cf170e",
			Conditions: []metav1.Condition{{
				Type:    "Ready",
				Status:  metav1.ConditionTrue,
				Reason:  "InstallSucceeded",
				Message: "Helm install succeeded",
			}},
		},
	}

	h := newHarness(t,
		&stubRegistry{tags: []string{"0.1.0-sha.8cf170e"}},
		newRelease(housebotv1alpha1.ReleaseSpec{
			Track:          "0.1.0-sha.*",
			HelmReleaseRef: &housebotv1alpha1.LocalObjectReference{Name: "housebot"},
		}),
		newOCIRepository(""),
		hr,
	)

	h.reconcile(t)

	release := h.release(t)
	if release.Status.HelmRelease == nil || !release.Status.HelmRelease.Ready {
		t.Fatalf("expected the HelmRelease to be mirrored ready, got %#v", release.Status.HelmRelease)
	}
	if release.Status.HelmRelease.Version != "0.1.0-sha.8cf170e" {
		t.Fatalf("mirrored version = %q", release.Status.HelmRelease.Version)
	}
	if ready := conditionOf(release, housebotv1alpha1.ConditionReady); ready.Status != metav1.ConditionTrue {
		t.Fatalf("Ready = %q", ready.Status)
	}
}

// The version was still written even when the HelmRelease cannot be read, so
// the Release reports not-ready rather than failed.
func TestMissingHelmReleaseStillDeploysTheVersion(t *testing.T) {
	h := newHarness(t,
		&stubRegistry{tags: []string{"0.1.0-sha.8cf170e"}},
		newRelease(housebotv1alpha1.ReleaseSpec{
			Track:          "0.1.0-sha.*",
			HelmReleaseRef: &housebotv1alpha1.LocalObjectReference{Name: "absent"},
		}),
		newOCIRepository(""),
	)

	h.reconcile(t)

	if got := h.ociTag(t); got != "0.1.0-sha.8cf170e" {
		t.Fatalf("the version must still be written, got %q", got)
	}
	release := h.release(t)
	if release.Status.Current != "0.1.0-sha.8cf170e" {
		t.Fatalf("status.current = %q", release.Status.Current)
	}
	if ready := conditionOf(release, housebotv1alpha1.ConditionReady); ready.Status != metav1.ConditionFalse {
		t.Fatalf("expected Ready=False, got %#v", ready)
	}
}

func TestReleaseWithNeitherTrackNorVersionIsRejected(t *testing.T) {
	h := newHarness(t,
		&stubRegistry{},
		newRelease(housebotv1alpha1.ReleaseSpec{}),
		newOCIRepository(""),
	)

	h.reconcile(t)

	ready := conditionOf(h.release(t), housebotv1alpha1.ConditionReady)
	if ready == nil || ready.Status != metav1.ConditionFalse {
		t.Fatalf("expected Ready=False, got %#v", ready)
	}
}

func TestReconcileRequeuesAtTheConfiguredInterval(t *testing.T) {
	release := newRelease(housebotv1alpha1.ReleaseSpec{Track: "0.1.0-sha.*"})
	release.Spec.Interval = metav1.Duration{Duration: 90 * 1e9}

	h := newHarness(t, &stubRegistry{tags: []string{"0.1.0-sha.a"}}, release, newOCIRepository(""))

	if got := h.reconcile(t).RequeueAfter; got != release.Spec.Interval.Duration {
		t.Fatalf("RequeueAfter = %v, want %v", got, release.Spec.Interval.Duration)
	}
}

func conditionOf(release *housebotv1alpha1.Release, conditionType string) *metav1.Condition {
	for i := range release.Status.Conditions {
		if release.Status.Conditions[i].Type == conditionType {
			return &release.Status.Conditions[i]
		}
	}
	return nil
}
