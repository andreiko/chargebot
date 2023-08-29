use serde::{Deserialize, Deserializer};

/// Park is a group of EV charging stations.
#[derive(Deserialize, Clone)]
pub struct Park {
    pub id: String,
    pub stations: Vec<Station>,
}

/// Station is an EV charging station indicating its availability status.
#[derive(Deserialize, Clone)]
pub struct Station {
    pub id: String,
    #[serde(default, deserialize_with = "Status::deserialize_fallback")]
    pub status: Status,
}

/// Status indicates current availability of an EV charging station.
#[derive(Default, Deserialize, Clone, PartialEq)]
pub enum Status {
    InUse,
    Available,
    #[default]
    Other,
}

impl Status {
    fn deserialize_fallback<'a, D: Deserializer<'a>>(deserializer: D) -> Result<Status, D::Error> {
        Ok(Status::deserialize(deserializer).unwrap_or(Status::default()))
    }
}
