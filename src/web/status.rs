use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;

pub const STATUS_CLIENT_SCRIPT: &str = concat!(
    "<script>\n",
    include_str!("status_client.js"),
    "\n</script>",
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Queued,
    Other(Cow<'static, str>),
}

impl JobStatus {
    pub fn as_str(&self) -> &str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Processing => "processing",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Queued => "queued",
            JobStatus::Other(value) => value.as_ref(),
        }
    }

    pub fn label_zh(&self) -> &str {
        match self {
            JobStatus::Pending => "待处理",
            JobStatus::Processing => "处理中",
            JobStatus::Completed => "已完成",
            JobStatus::Failed => "已失败",
            JobStatus::Queued => "排队中",
            JobStatus::Other(value) => value.as_ref(),
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "pending" => JobStatus::Pending,
            "processing" => JobStatus::Processing,
            "completed" => JobStatus::Completed,
            "failed" => JobStatus::Failed,
            "queued" => JobStatus::Queued,
            other => JobStatus::Other(Cow::Owned(other.to_string())),
        }
    }
}

impl Serialize for JobStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for JobStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(JobStatus::from_str(&value))
    }
}
