use crate::discovery::Candidate;
use crate::models::ArtifactReport;

pub trait Detector {
    fn name(&self) -> &str;
    fn detect(&self, candidates: &[Candidate], deep: bool) -> Vec<ArtifactReport>;
}
