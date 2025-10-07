use serde::Deserialize;

#[derive(Default, Deserialize)]
pub struct DashboardQuery {
    pub status: Option<String>,
    pub error: Option<String>,
}
