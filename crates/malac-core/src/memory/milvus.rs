//! Milvus RESTful API (v2). Works with self-hosted Milvus and Zilliz Cloud.

use anyhow::{Result, bail};
use serde_json::{Value, json};

pub struct Milvus {
    http: reqwest::Client,
    base_url: String,
    token: Option<String>,
    database: Option<String>,
    collection: String,
}

impl Milvus {
    pub fn new(
        url: &str,
        token: Option<String>,
        database: Option<String>,
        collection: &str,
    ) -> Self {
        Self {
            http: crate::provider::http_client(),
            base_url: url.trim_end_matches('/').to_owned(),
            token,
            database,
            collection: collection.to_owned(),
        }
    }

    async fn call(&self, path: &str, mut body: Value) -> Result<Value> {
        body["collectionName"] = self.collection.clone().into();
        if let Some(db) = &self.database {
            body["dbName"] = db.clone().into();
        }
        let mut req = self
            .http
            .post(format!("{}/v2/vectordb/{path}", self.base_url))
            .json(&body);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let resp: Value = req.send().await?.error_for_status()?.json().await?;
        match resp["code"].as_i64() {
            Some(0) | Some(200) => Ok(resp["data"].clone()),
            _ => bail!(
                "Milvus {path} failed: {}",
                resp["message"].as_str().unwrap_or("unknown error")
            ),
        }
    }

    pub async fn ensure_collection(&self, dimension: usize) -> Result<()> {
        let exists = self.call("collections/has", json!({})).await?;
        if exists["has"].as_bool() == Some(true) {
            return Ok(());
        }
        let varchar = |name: &str, max: u32, primary: bool| {
            json!({
                "fieldName": name,
                "dataType": "VarChar",
                "isPrimary": primary,
                "elementTypeParams": { "max_length": max.to_string() },
            })
        };
        let schema = json!({
            "autoId": false,
            "enableDynamicField": false,
            "fields": [
                varchar("id", 1024, true),
                { "fieldName": "vector", "dataType": "FloatVector", "elementTypeParams": { "dim": dimension.to_string() } },
                varchar("scope", 16, false),
                varchar("project", 1024, false),
                varchar("name", 256, false),
            ],
        });
        let index = json!([{
            "fieldName": "vector",
            "indexName": "vector",
            "metricType": "COSINE",
            "indexType": "AUTOINDEX",
        }]);
        self.call(
            "collections/create",
            json!({ "schema": schema, "indexParams": index }),
        )
        .await?;
        Ok(())
    }

    pub async fn upsert(&self, rows: Vec<Value>) -> Result<()> {
        self.call("entities/upsert", json!({ "data": rows }))
            .await?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let filter = format!("id in [{}]", serde_json::to_string(id)?);
        self.call("entities/delete", json!({ "filter": filter }))
            .await?;
        Ok(())
    }

    /// Returns `(name, scope, similarity)` for the nearest rows.
    pub async fn search(
        &self,
        vector: Vec<f32>,
        limit: usize,
        filter: &str,
    ) -> Result<Vec<(String, String, f64)>> {
        let data = self
            .call(
                "entities/search",
                json!({
                    "data": [vector],
                    "annsField": "vector",
                    "limit": limit,
                    "filter": filter,
                    "outputFields": ["name", "scope"],
                }),
            )
            .await?;
        Ok(data
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| {
                Some((
                    row["name"].as_str()?.to_owned(),
                    row["scope"].as_str()?.to_owned(),
                    row["distance"].as_f64().unwrap_or(0.0),
                ))
            })
            .collect())
    }
}
