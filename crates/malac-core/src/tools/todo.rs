use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput};
use crate::event::{Event, Todo, TodoStatus};

pub struct Todos;

#[async_trait]
impl Tool for Todos {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> String {
        "Replace your task list for the current request. Use it for work with three or more steps: \
         keep exactly one item in_progress, mark items completed as soon as they are done, and send \
         the whole list every time."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["items"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        let items = input["items"].as_array().map_or(0, Vec::len);
        format!("{items} items")
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let items: Vec<Todo> = match serde_json::from_value(input["items"].clone()) {
            Ok(items) => items,
            Err(err) => return ToolOutput::error(format!("invalid items: {err}")),
        };
        let rendered: Vec<String> = items
            .iter()
            .map(|t| {
                let mark = match t.status {
                    TodoStatus::Pending => "[ ]",
                    TodoStatus::InProgress => "[~]",
                    TodoStatus::Completed => "[x]",
                };
                format!("{mark} {}", t.content)
            })
            .collect();
        ctx.emit(Event::Todos { items });
        ToolOutput::text(rendered.join("\n"))
    }
}
