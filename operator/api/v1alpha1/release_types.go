package v1alpha1

import (
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// DefaultHistoryLimit bounds status.history. etcd is not a backup; this is a
// convenience ledger deep enough to roll back through a bad afternoon.
const DefaultHistoryLimit = 10

const (
	// SourceAuto marks a version the track poller chose.
	SourceAuto = "auto"
	// SourcePin marks a version an operator pinned through spec.version.
	SourcePin = "pin"
)

// Condition types reported on a Release.
const (
	ConditionReady    = "Ready"
	ConditionResolved = "Resolved"
)

// OCIRepositoryReference names the Flux OCIRepository whose tag this Release
// owns. The operator patches exactly this one object and nothing else.
type OCIRepositoryReference struct {
	// +kubebuilder:validation:MinLength=1
	Name string `json:"name"`

	// Defaults to the Release's own namespace.
	// +optional
	Namespace string `json:"namespace,omitempty"`
}

// LocalObjectReference names an object in the Release's namespace.
type LocalObjectReference struct {
	// +kubebuilder:validation:MinLength=1
	Name string `json:"name"`
}

type ReleaseSpec struct {
	// Glob matched against the chart's tags in the registry, such as
	// "0.1.0-sha.*". Ignored while version is set.
	// +optional
	Track string `json:"track,omitempty"`

	// An explicit chart version. Setting it pins the Release: track following
	// stops until it is cleared again. This is what makes a rollback stick —
	// without the pin the poller would see the newer, broken chart still in the
	// registry and redeploy it on the next tick.
	// +optional
	Version string `json:"version,omitempty"`

	// The Flux OCIRepository to patch.
	OCIRepositoryRef OCIRepositoryReference `json:"ociRepositoryRef"`

	// The HelmRelease whose readiness is mirrored into status. Optional: without
	// it the Release reports only what it resolved, not whether it landed.
	// +optional
	HelmReleaseRef *LocalObjectReference `json:"helmReleaseRef,omitempty"`

	// How often the registry is polled while following a track.
	// +kubebuilder:default="2m"
	// +optional
	Interval metav1.Duration `json:"interval,omitempty"`

	// Entries kept in status.history.
	// +kubebuilder:default=10
	// +kubebuilder:validation:Minimum=1
	// +optional
	HistoryLimit *int32 `json:"historyLimit,omitempty"`
}

// HistoryEntry records one version this Release deployed.
type HistoryEntry struct {
	Version string      `json:"version"`
	At      metav1.Time `json:"at"`
	// +kubebuilder:validation:Enum=auto;pin
	By string `json:"by"`
}

// HelmReleaseStatus mirrors the readiness of the HelmRelease Flux drives from
// the OCIRepository this Release patches.
type HelmReleaseStatus struct {
	Ready bool `json:"ready"`
	// +optional
	Message string `json:"message,omitempty"`
	// +optional
	Version string `json:"version,omitempty"`
}

type ReleaseStatus struct {
	// The chart version currently written to the OCIRepository.
	// +optional
	Current string `json:"current,omitempty"`

	// True while spec.version holds the Release off its track.
	// +optional
	Pinned bool `json:"pinned,omitempty"`

	// Newest first, bounded by spec.historyLimit.
	// +optional
	History []HistoryEntry `json:"history,omitempty"`

	// +optional
	HelmRelease *HelmReleaseStatus `json:"helmRelease,omitempty"`

	// +optional
	ObservedGeneration int64 `json:"observedGeneration,omitempty"`

	// +optional
	// +patchMergeKey=type
	// +patchStrategy=merge
	// +listType=map
	// +listMapKey=type
	Conditions []metav1.Condition `json:"conditions,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Namespaced,shortName=rel
// +kubebuilder:printcolumn:name="Current",type=string,JSONPath=`.status.current`
// +kubebuilder:printcolumn:name="Pinned",type=boolean,JSONPath=`.status.pinned`
// +kubebuilder:printcolumn:name="Track",type=string,JSONPath=`.spec.track`
// +kubebuilder:printcolumn:name="Ready",type=string,JSONPath=`.status.conditions[?(@.type=="Ready")].status`
// +kubebuilder:printcolumn:name="Age",type=date,JSONPath=`.metadata.creationTimestamp`

// Release declares which version of a chart should be running. The version
// lives here rather than in git so that a deploy is a field patch instead of a
// commit.
type Release struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec ReleaseSpec `json:"spec,omitempty"`
	// +optional
	Status ReleaseStatus `json:"status,omitempty"`
}

// +kubebuilder:object:root=true

type ReleaseList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Release `json:"items"`
}

func init() {
	SchemeBuilder.Register(&Release{}, &ReleaseList{})
}

// HistoryLimitOrDefault resolves the bound on status.history.
func (r *Release) HistoryLimitOrDefault() int {
	if r.Spec.HistoryLimit == nil || *r.Spec.HistoryLimit < 1 {
		return DefaultHistoryLimit
	}
	return int(*r.Spec.HistoryLimit)
}

// OCIRepositoryNamespace resolves the namespace of the patched OCIRepository.
func (r *Release) OCIRepositoryNamespace() string {
	if r.Spec.OCIRepositoryRef.Namespace != "" {
		return r.Spec.OCIRepositoryRef.Namespace
	}
	return r.Namespace
}
