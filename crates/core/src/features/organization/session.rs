use crate::features::organization::metrics::IntegrityReport;
use crate::features::organization::{engine::OrganizationPlan, GameMetadata, OrganizationRule};
use crate::ArchiveEntry;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct OrganizationSession {
    pub archive_name: String,
    pub entries: Vec<ArchiveEntry>,
    pub rules: Vec<OrganizationRule>,
    pub metadata: Option<GameMetadata>,
    pub preview_plan: Option<OrganizationPlan>,
    pub network_log: Vec<(SystemTime, String)>,
}

impl OrganizationSession {
    pub fn new(
        archive_name: String,
        entries: Vec<ArchiveEntry>,
        rules: Vec<OrganizationRule>,
        metadata: Option<GameMetadata>,
    ) -> Self {
        Self {
            archive_name,
            entries,
            rules,
            metadata,
            preview_plan: None,
            network_log: Vec::new(),
        }
    }

    pub fn set_plan(&mut self, plan: OrganizationPlan) {
        self.preview_plan = Some(plan);
    }

    pub fn clear_plan(&mut self) {
        self.preview_plan = None;
    }

    pub fn add_log(&mut self, message: String) {
        self.network_log.push((SystemTime::now(), message));
    }

    pub fn calculate_report(&self) -> IntegrityReport {
        IntegrityReport::calculate(
            &self.entries,
            self.preview_plan.as_ref(),
            self.metadata.as_ref(),
        )
    }
}
