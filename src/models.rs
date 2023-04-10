use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct Access {
    pub id: i32,
    pub link_id: i32,
    pub address: String,
    pub accessed_at: DateTime<Utc>,
}

#[derive(Deserialize, Debug)]
pub struct NewLink {
    pub author: i64,
    pub target: String,
    pub title: String,
}

#[derive(Serialize, Debug)]
pub struct Link {
    pub id: i32,
    pub author: i64,
    pub target: String,
    pub title: String,
}
