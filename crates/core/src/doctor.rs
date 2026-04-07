pub fn run_doctor(palace_path: &std::path::Path) -> anyhow::Result<DoctorReport> {
    let _ = palace_path;
    Ok(DoctorReport {
        checks: vec![],
        healthy: true,
    })
}

#[derive(Debug)]
pub struct DoctorReport {
    pub checks: Vec<CheckResult>,
    pub healthy: bool,
}

#[derive(Debug)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}
