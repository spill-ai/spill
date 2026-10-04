use crate::db::Store;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone)]
pub struct SpillServer {
    store: Store,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryInput {
    pub sql: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeInput {
    pub dataset: String,
}

fn response(result: anyhow::Result<Value>) -> CallToolResult {
    match result {
        Ok(value) => {
            let result = CallToolResult::structured(serde_json::json!({"result": value}));
            // `structured` encodes the value twice (text + structuredContent).
            // The inner store methods already cap data at MAX_BYTES (32 KiB); the
            // envelope check here is a safety net for describe/list overhead only.
            // Threshold is 2× to account for both encodings plus framing.
            if serde_json::to_vec(&result)
                .map_or(true, |b| b.len() > crate::db::MAX_BYTES * 2 + 512)
            {
                CallToolResult::error(vec![Content::text(
                    "MCP result too large. Select fewer columns, add LIMIT, or aggregate.",
                )])
            } else {
                result
            }
        }
        Err(error) => {
            let message: String = error.to_string().chars().take(512).collect();
            CallToolResult::error(vec![Content::text(message)])
        }
    }
}

#[tool_router]
impl SpillServer {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Run read-only SQL on local Spill datasets. Maximum 500 rows and 32 KiB of result data. Use LIMIT, filtering and aggregates. Rows are arrays matching the columns field. Nested values are JSON-encoded VARCHAR."
    )]
    async fn query(
        &self,
        Parameters(input): Parameters<QueryInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(response(self.store.query(&input.sql)))
    }
    #[tool(
        description = "List local Spill datasets with row counts, source tools and creation timestamps."
    )]
    async fn list(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(response(self.store.list()))
    }

    #[tool(
        description = "Describe a Spill dataset's columns, DuckDB types, row count and source metadata without returning its rows."
    )]
    async fn describe(
        &self,
        Parameters(input): Parameters<DescribeInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(response(self.store.describe(&input.dataset)))
    }
}

#[tool_handler]
impl ServerHandler for SpillServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Query spilled MCP results locally using SQL. Start with list or describe. Never select entire large datasets; filter, aggregate, and LIMIT results.".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation { name: "spill".into(), version: env!("CARGO_PKG_VERSION").into(), ..Default::default() },
            ..Default::default()
        }
    }
}

pub async fn serve(store: Store) -> anyhow::Result<()> {
    let service = SpillServer::new(store)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
