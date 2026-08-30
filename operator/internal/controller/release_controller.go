// Package controller reconciles Release resources onto Flux sources.
package controller

import (
	"context"
	"fmt"
	"strings"
	"time"

	helmv2 "github.com/fluxcd/helm-controller/api/v2"
	sourcev1 "github.com/fluxcd/source-controller/api/v1"
	apiequality "k8s.io/apimachinery/pkg/api/equality"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	"sigs.k8s.io/controller-runtime/pkg/log"

	housebotv1alpha1 "github.com/bushshrub/housebot/operator/api/v1alpha1"
)

// DefaultInterval is how often a track is re-resolved when the Release does not
// say.
const DefaultInterval = 2 * time.Minute

// Resolver is the registry side of reconciliation, kept as an interface so the
// controller can be tested without a registry.
type Resolver interface {
	Resolve(ctx context.Context, repo, track string) (string, error)
	Exists(ctx context.Context, repo, tag string) (bool, error)
}

type ReleaseReconciler struct {
	client.Client
	Scheme   *runtime.Scheme
	Registry Resolver
}

// +kubebuilder:rbac:groups=housebot.dev,resources=releases,verbs=get;list;watch
// +kubebuilder:rbac:groups=housebot.dev,resources=releases/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=source.toolkit.fluxcd.io,resources=ocirepositories,verbs=get;list;watch;patch;update
// +kubebuilder:rbac:groups=helm.toolkit.fluxcd.io,resources=helmreleases,verbs=get;list;watch
// +kubebuilder:rbac:groups="",resources=events,verbs=create;patch

func (r *ReleaseReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	var release housebotv1alpha1.Release
	if err := r.Get(ctx, req.NamespacedName, &release); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	interval := release.Spec.Interval.Duration
	if interval <= 0 {
		interval = DefaultInterval
	}

	patchBase := release.DeepCopy()
	result := ctrl.Result{RequeueAfter: interval}

	if err := r.reconcile(ctx, &release); err != nil {
		logger.Error(err, "reconcile failed")
		setCondition(&release, housebotv1alpha1.ConditionReady, metav1.ConditionFalse, "Error", err.Error())
		// The status write still has to happen, so the error is reported through
		// the condition rather than returned and retried blindly.
		if statusErr := r.patchStatus(ctx, patchBase, &release); statusErr != nil {
			return ctrl.Result{}, statusErr
		}
		return result, nil
	}

	return result, r.patchStatus(ctx, patchBase, &release)
}

func (r *ReleaseReconciler) reconcile(ctx context.Context, release *housebotv1alpha1.Release) error {
	logger := log.FromContext(ctx)

	repoKey := types.NamespacedName{
		Name:      release.Spec.OCIRepositoryRef.Name,
		Namespace: release.OCIRepositoryNamespace(),
	}
	var ociRepo sourcev1.OCIRepository
	if err := r.Get(ctx, repoKey, &ociRepo); err != nil {
		return fmt.Errorf("get OCIRepository %s: %w", repoKey, err)
	}

	// The chart's registry address is already declared on the OCIRepository, so
	// the Release does not restate it and the two cannot drift apart.
	chartRepo := strings.TrimPrefix(ociRepo.Spec.URL, "oci://")
	if chartRepo == "" {
		return fmt.Errorf("OCIRepository %s has no url", repoKey)
	}

	desired, source, err := r.resolve(ctx, release, chartRepo)
	if err != nil {
		return err
	}

	release.Status.Pinned = source == housebotv1alpha1.SourcePin
	setCondition(release, housebotv1alpha1.ConditionResolved, metav1.ConditionTrue, "Resolved",
		fmt.Sprintf("resolved %s from %s", desired, describeSource(release)))

	if tagOf(&ociRepo) != desired {
		logger.Info("patching OCIRepository", "repository", repoKey, "from", tagOf(&ociRepo), "to", desired)
		patch := client.MergeFrom(ociRepo.DeepCopy())
		if ociRepo.Spec.Reference == nil {
			ociRepo.Spec.Reference = &sourcev1.OCIRepositoryRef{}
		}
		ociRepo.Spec.Reference.Tag = desired
		// A digest or semver range on the same object would silently outrank the
		// tag, leaving the Release convinced it had deployed something it had not.
		ociRepo.Spec.Reference.Digest = ""
		ociRepo.Spec.Reference.SemVer = ""
		if err := r.Patch(ctx, &ociRepo, patch); err != nil {
			return fmt.Errorf("patch OCIRepository %s: %w", repoKey, err)
		}
	}

	if release.Status.Current != desired {
		release.Status.History = prependHistory(release.Status.History, housebotv1alpha1.HistoryEntry{
			Version: desired,
			At:      metav1.Now(),
			By:      source,
		}, release.HistoryLimitOrDefault())
		release.Status.Current = desired
	}

	r.mirrorHelmRelease(ctx, release)
	release.Status.ObservedGeneration = release.Generation
	return nil
}

// resolve decides which chart version should be running. An explicit
// spec.version always wins, which is what suspends track following and makes a
// rollback survive the next poll.
func (r *ReleaseReconciler) resolve(
	ctx context.Context,
	release *housebotv1alpha1.Release,
	chartRepo string,
) (string, string, error) {
	if v := strings.TrimSpace(release.Spec.Version); v != "" {
		exists, err := r.Registry.Exists(ctx, chartRepo, v)
		if err != nil {
			return "", "", fmt.Errorf("check pinned version %q: %w", v, err)
		}
		if !exists {
			return "", "", fmt.Errorf("pinned version %q is not in %s", v, chartRepo)
		}
		return v, housebotv1alpha1.SourcePin, nil
	}

	if release.Spec.Track == "" {
		return "", "", fmt.Errorf("set spec.version or spec.track")
	}

	resolved, err := r.Registry.Resolve(ctx, chartRepo, release.Spec.Track)
	if err != nil {
		return "", "", err
	}
	return resolved, housebotv1alpha1.SourceAuto, nil
}

// mirrorHelmRelease copies Flux's own verdict into the Release. It is advisory:
// a missing or unreadable HelmRelease leaves the Release resolved but not ready,
// never failed, because the version was still written successfully.
func (r *ReleaseReconciler) mirrorHelmRelease(ctx context.Context, release *housebotv1alpha1.Release) {
	if release.Spec.HelmReleaseRef == nil {
		setCondition(release, housebotv1alpha1.ConditionReady, metav1.ConditionTrue, "Resolved",
			fmt.Sprintf("deploying %s", release.Status.Current))
		return
	}

	key := types.NamespacedName{Name: release.Spec.HelmReleaseRef.Name, Namespace: release.Namespace}
	var hr helmv2.HelmRelease
	if err := r.Get(ctx, key, &hr); err != nil {
		message := fmt.Sprintf("HelmRelease %s unreadable: %v", key, err)
		if apierrors.IsNotFound(err) {
			message = fmt.Sprintf("HelmRelease %s not found", key)
		}
		release.Status.HelmRelease = &housebotv1alpha1.HelmReleaseStatus{Message: message}
		setCondition(release, housebotv1alpha1.ConditionReady, metav1.ConditionFalse, "HelmReleaseUnavailable", message)
		return
	}

	ready := metav1.ConditionUnknown
	message := "awaiting HelmRelease"
	for _, condition := range hr.Status.Conditions {
		if condition.Type == "Ready" {
			ready = condition.Status
			message = condition.Message
			break
		}
	}

	release.Status.HelmRelease = &housebotv1alpha1.HelmReleaseStatus{
		Ready:   ready == metav1.ConditionTrue,
		Message: message,
		Version: hr.Status.LastAttemptedRevision,
	}
	setCondition(release, housebotv1alpha1.ConditionReady, ready, "HelmRelease", message)
}

func (r *ReleaseReconciler) patchStatus(
	ctx context.Context,
	base *housebotv1alpha1.Release,
	release *housebotv1alpha1.Release,
) error {
	// Poll ticks that change nothing must not write, or every Release would
	// generate a status update every interval forever.
	if apiequality.Semantic.DeepEqual(base.Status, release.Status) {
		return nil
	}
	return r.Status().Patch(ctx, release, client.MergeFrom(base))
}

func tagOf(repo *sourcev1.OCIRepository) string {
	if repo.Spec.Reference == nil {
		return ""
	}
	return repo.Spec.Reference.Tag
}

func describeSource(release *housebotv1alpha1.Release) string {
	if release.Status.Pinned {
		return "pin"
	}
	return fmt.Sprintf("track %q", release.Spec.Track)
}

// prependHistory puts the newest entry first and drops the oldest past limit.
func prependHistory(
	history []housebotv1alpha1.HistoryEntry,
	entry housebotv1alpha1.HistoryEntry,
	limit int,
) []housebotv1alpha1.HistoryEntry {
	out := append([]housebotv1alpha1.HistoryEntry{entry}, history...)
	if len(out) > limit {
		out = out[:limit]
	}
	return out
}

func setCondition(
	release *housebotv1alpha1.Release,
	conditionType string,
	status metav1.ConditionStatus,
	reason, message string,
) {
	condition := metav1.Condition{
		Type:               conditionType,
		Status:             status,
		Reason:             reason,
		Message:            truncate(message, 32768),
		ObservedGeneration: release.Generation,
		LastTransitionTime: metav1.Now(),
	}

	for i, existing := range release.Status.Conditions {
		if existing.Type != conditionType {
			continue
		}
		if existing.Status == status {
			condition.LastTransitionTime = existing.LastTransitionTime
		}
		release.Status.Conditions[i] = condition
		return
	}
	release.Status.Conditions = append(release.Status.Conditions, condition)
}

func truncate(s string, max int) string {
	if len(s) <= max {
		return s
	}
	return s[:max]
}

func (r *ReleaseReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&housebotv1alpha1.Release{}).
		// A HelmRelease going ready is what makes the mirrored status true, so
		// waiting out the poll interval to notice would make every deploy look
		// slower than it was.
		Watches(&helmv2.HelmRelease{}, handler.EnqueueRequestsFromMapFunc(r.releasesForHelmRelease)).
		Named("release").
		Complete(r)
}

func (r *ReleaseReconciler) releasesForHelmRelease(ctx context.Context, obj client.Object) []ctrl.Request {
	var releases housebotv1alpha1.ReleaseList
	if err := r.List(ctx, &releases, client.InNamespace(obj.GetNamespace())); err != nil {
		return nil
	}

	var requests []ctrl.Request
	for _, release := range releases.Items {
		if release.Spec.HelmReleaseRef == nil || release.Spec.HelmReleaseRef.Name != obj.GetName() {
			continue
		}
		requests = append(requests, ctrl.Request{
			NamespacedName: types.NamespacedName{Name: release.Name, Namespace: release.Namespace},
		})
	}
	return requests
}
