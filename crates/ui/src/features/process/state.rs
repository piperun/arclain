//! Process page state — the pipeline draft the editor mutates, the
//! preview the application computed for it, run status, and presets.
//!
//! Everything here speaks the application's own pipeline vocabulary
//! (`arclain_app::operations::pipeline`'s DTOs) rather than
//! `arclain_core::Pipeline`. The draft below is deliberately shaped so
//! that [`ProcessPageState::preview_request`] can assemble a
//! [`PipelinePreviewRequest`] from it with no further decisions, because
//! that request converts into the run request the page then dispatches
//! (`PipelineRequest::from`) — the preview and the run are one
//! description, not two that have to be kept in step by hand.
//!
//! ## No metadata here, deliberately
//!
//! The pre-facade page carried a `last_previewed_metadata_key` and fed
//! the active tab's plugin-reported `GameMetadata` into every preview.
//! That predicted one name for a whole batch, because one blob applied
//! to N inputs derives the same output stem N times, while the run
//! resolves each input's metadata separately. `preview_pipeline` now
//! resolves metadata per input exactly as the executor does and takes no
//! metadata parameter at all, so there is nothing for this state to
//! carry and nothing to invalidate a cached preview on.

use arclain_app::operations::pipeline::{
    OutputArtifactDto, OutputCollisionPolicyDto, PipelineDestinationDto, PipelineInputsDto,
    PipelineSpecDto, PipelineStepDto,
};
use arclain_app::organization::OrganizationRuleSummary;
use arclain_app::process::{PipelinePresetSummary, PipelinePreviewDto, PipelinePreviewRequest};

/// How many interrupted runs the banner asks the application for.
///
/// `interrupted_pipeline_runs` bounds the *answer*, not the query, and
/// nothing ever clears an interrupted run — so asking from `0` returns
/// every one ever recorded in this profile and that set only grows. The
/// banner therefore reports a bounded count and says so when it is
/// saturated (see [`ProcessPageState::interrupted_run_label`]) rather
/// than pretending the number is exact.
pub const INTERRUPTED_RUN_QUERY_LIMIT: u32 = 50;

/// The editable pipeline the Process page builds, in the application's
/// own request vocabulary.
///
/// Split out from [`ProcessPageState`] so the whole draft can be
/// replaced wholesale when a preset is applied, without disturbing the
/// page's caches (presets list, organization rules, run status).
#[derive(Clone, Debug, PartialEq)]
pub struct PipelineDraft {
    pub inputs: PipelineInputsDto,
    pub steps: Vec<PipelineStepDto>,
    pub destination: PipelineDestinationDto,
    pub collision_policy: Option<OutputCollisionPolicyDto>,
    pub output_artifact: OutputArtifactDto,
}

impl Default for PipelineDraft {
    fn default() -> Self {
        Self {
            // "No input selected" is an empty file list rather than an
            // `Option`: a preview accepts empty inputs and answers with
            // its own global warning, so the page has nothing to
            // special-case.
            inputs: PipelineInputsDto::Files { paths: Vec::new() },
            steps: Vec::new(),
            destination: PipelineDestinationDto::SameFolder,
            collision_policy: None,
            output_artifact: OutputArtifactDto::default(),
        }
    }
}

impl PipelineDraft {
    /// True when no input has been chosen yet.
    pub fn has_no_input(&self) -> bool {
        matches!(&self.inputs, PipelineInputsDto::Files { paths } if paths.is_empty())
    }
}

#[derive(Default)]
pub struct ProcessPageState {
    pub draft: PipelineDraft,
    /// The application's answer for the current draft. Recomputed
    /// through `ArclainApp::preview_pipeline` whenever
    /// [`Self::preview_dirty`] is set — never derived here, so what the
    /// page shows is what the run was told. Empty when the last preview
    /// was *refused* rather than answered; see [`Self::preview_error`].
    pub preview: PipelinePreviewDto,
    /// Why the application refused to preview the draft at all, if it
    /// did — in practice an Organize step whose rule has not been picked
    /// yet, which is what the "+ Organize" button produces.
    ///
    /// Deliberately its own field rather than an extra entry pushed into
    /// [`Self::preview`]'s `global_warnings`. A warning is something the
    /// application *said about* a pipeline it successfully described; a
    /// refusal is the application declining to describe it at all, and
    /// the two have different consequences (a warned pipeline is
    /// runnable, a refused one is not). Forging a `PipelinePreviewDto`
    /// to carry the refusal would make them indistinguishable to
    /// everything downstream — including a test that could then no
    /// longer tell "the application warned" from "the page invented a
    /// warning".
    pub preview_error: Option<String>,
    pub preview_dirty: bool,
    pub is_running: bool,
    pub last_result_summary: Option<String>,
    /// Saved presets as the application reports them. `None` = not yet
    /// loaded; the page emits a `LoadPresets` action on first render.
    pub presets: Option<Vec<PipelinePresetSummary>>,
    pub active_preset_name: Option<String>,
    /// Count of interrupted pipeline runs the application reported,
    /// bounded by [`INTERRUPTED_RUN_QUERY_LIMIT`]. Shown as a banner
    /// until the user dismisses it. `None` = not yet queried.
    pub interrupted_run_count: Option<usize>,
    pub interrupted_banner_dismissed: bool,
    /// Cached organization rules for the Organize step picker. `None` =
    /// not yet loaded; the page emits a LoadOrganizationRules action on
    /// the first render to populate this. Cached for the session — if
    /// the user adds rules in Settings, they'll see them on next launch
    /// (or a future explicit refresh).
    pub cached_org_rules: Option<Vec<OrganizationRuleSummary>>,
}

impl ProcessPageState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_dirty(&mut self) {
        self.preview_dirty = true;
    }

    /// The one description the preview and the run share.
    ///
    /// [`PipelineRequest::from`](arclain_app::operations::PipelineRequest)
    /// turns exactly this into the run request, so the page never
    /// assembles a second description of the same pipeline.
    pub fn preview_request(&self) -> PipelinePreviewRequest {
        PipelinePreviewRequest {
            inputs: self.draft.inputs.clone(),
            destination: self.draft.destination.clone(),
            pipeline: PipelineSpecDto::Steps {
                steps: self.draft.steps.clone(),
                output_artifact: self.draft.output_artifact,
            },
            collision_policy: self.draft.collision_policy,
        }
    }

    /// Applies a saved preset: its steps, destination, collision policy
    /// and output artifact replace the draft's, while the currently
    /// chosen input is preserved (the pre-facade page did the same —
    /// a preset describes *what to do*, not *what to do it to*, and a
    /// saved preset carries no input at all through this facade).
    pub fn apply_preset(&mut self, preset: &PipelinePresetSummary) {
        self.draft.steps = preset.steps.clone();
        self.draft.destination = preset.destination.clone();
        self.draft.collision_policy = preset.collision_policy;
        self.draft.output_artifact = preset.output_artifact;
        self.active_preset_name = Some(preset.name.clone());
        self.mark_dirty();
    }

    /// Presets as the page renders them; empty until the first
    /// `LoadPresets` dispatch lands.
    pub fn presets(&self) -> &[PipelinePresetSummary] {
        self.presets.as_deref().unwrap_or_default()
    }

    /// How the interrupted-run banner names its count.
    ///
    /// Saturating the query limit is reported as "N+" rather than as an
    /// exact N: the application bounds the answer, not the query, so a
    /// full page means "at least this many", and claiming otherwise
    /// would be a number the page cannot stand behind.
    pub fn interrupted_run_label(&self) -> String {
        let count = self.interrupted_run_count.unwrap_or(0);
        if count >= INTERRUPTED_RUN_QUERY_LIMIT as usize {
            format!("{count}+")
        } else {
            count.to_string()
        }
    }
}
