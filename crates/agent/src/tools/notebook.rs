//! NotebookEditTool — edit Jupyter `.ipynb` notebooks.
//! Port of `nanobot.agent.tools.notebook`.

use std::fs;

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::base::{Tool, ToolExecError};
use super::filesystem::FsTool;

pub struct NotebookEditTool(pub FsTool);

impl NotebookEditTool {
    fn new_cell(source: &str, cell_type: &str, generate_id: bool) -> Value {
        let mut cell = Map::new();
        cell.insert("cell_type".into(), Value::String(cell_type.into()));
        cell.insert("source".into(), Value::String(source.into()));
        cell.insert("metadata".into(), Value::Object(Map::new()));
        if cell_type == "code" {
            cell.insert("outputs".into(), Value::Array(Vec::new()));
            cell.insert("execution_count".into(), Value::Null);
        }
        if generate_id {
            cell.insert(
                "id".into(),
                Value::String(Uuid::new_v4().simple().to_string()[..8].into()),
            );
        }
        Value::Object(cell)
    }

    fn empty_notebook() -> Value {
        json!({
            "nbformat": 4,
            "nbformat_minor": 5,
            "metadata": {
                "kernelspec": {"display_name":"Python 3","language":"python","name":"python3"},
                "language_info": {"name":"python"},
            },
            "cells": [],
        })
    }
}

#[async_trait]
impl Tool for NotebookEditTool {
    fn name(&self) -> &str {
        "notebook_edit"
    }
    fn description(&self) -> String {
        "Edit a Jupyter notebook (.ipynb) cell. Modes: replace (default), insert (after target), delete. cell_index is 0-based.".into()
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "path":{"type":"string","description":"Path to the .ipynb notebook"},
                "cell_index":{"type":"integer","minimum":0,"description":"0-based cell index"},
                "new_source":{"type":"string","description":"New source content"},
                "cell_type":{"type":"string","enum":["code","markdown"],"description":"Cell type (default code)"},
                "edit_mode":{"type":"string","enum":["replace","insert","delete"],"description":"Edit mode"},
            },
            "required":["path","cell_index"],
        })
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let Some(path) = params.get("path").and_then(|v| v.as_str()) else {
            return Ok(Value::String("Error: path is required".into()));
        };
        if !path.ends_with(".ipynb") {
            return Ok(Value::String(
                "Error: notebook_edit only works on .ipynb files. Use edit_file for other files."
                    .into(),
            ));
        }
        let cell_index = params
            .get("cell_index")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let new_source = params
            .get("new_source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let cell_type = params
            .get("cell_type")
            .and_then(|v| v.as_str())
            .unwrap_or("code");
        let edit_mode = params
            .get("edit_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("replace");
        if !matches!(edit_mode, "replace" | "insert" | "delete") {
            return Ok(Value::String(format!(
                "Error: Invalid edit_mode '{edit_mode}'. Use one of: replace, insert, delete."
            )));
        }
        if !matches!(cell_type, "code" | "markdown") {
            return Ok(Value::String(format!(
                "Error: Invalid cell_type '{cell_type}'. Use one of: code, markdown."
            )));
        }

        let fp = match self.0.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(Value::String(format!("Error: {e}"))),
        };

        if !fp.exists() {
            if edit_mode != "insert" {
                return Ok(Value::String(format!("Error: File not found: {path}")));
            }
            let mut nb = Self::empty_notebook();
            let cell = Self::new_cell(new_source, cell_type, true);
            if let Some(cells) = nb.get_mut("cells").and_then(|v| v.as_array_mut()) {
                cells.push(cell);
            }
            if let Some(parent) = fp.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let serialized = serde_json::to_string_pretty(&nb).unwrap_or_default();
            if let Err(e) = fs::write(&fp, serialized) {
                return Ok(Value::String(format!("Error editing notebook: {e}")));
            }
            return Ok(Value::String(format!(
                "Successfully created {} with 1 cell",
                fp.display()
            )));
        }

        let raw = match fs::read_to_string(&fp) {
            Ok(r) => r,
            Err(e) => return Ok(Value::String(format!("Error: {e}"))),
        };
        let mut nb: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Value::String(format!(
                    "Error: Failed to parse notebook: {e}"
                )));
            }
        };
        let nbformat = nb.get("nbformat").and_then(|v| v.as_i64()).unwrap_or(0);
        let nbformat_minor = nb
            .get("nbformat_minor")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let generate_id = nbformat >= 4 && nbformat_minor >= 5;

        let cells = nb
            .get_mut("cells")
            .and_then(|v| v.as_array_mut())
            .ok_or_else(|| ToolExecError::Other("notebook has no 'cells' array".into()))?;

        match edit_mode {
            "delete" => {
                if cell_index < 0 || (cell_index as usize) >= cells.len() {
                    return Ok(Value::String(format!(
                        "Error: cell_index {cell_index} out of range (notebook has {} cells)",
                        cells.len()
                    )));
                }
                cells.remove(cell_index as usize);
                let serialized = serde_json::to_string_pretty(&nb).unwrap_or_default();
                if let Err(e) = fs::write(&fp, serialized) {
                    return Ok(Value::String(format!("Error editing notebook: {e}")));
                }
                Ok(Value::String(format!(
                    "Successfully deleted cell {cell_index} from {}",
                    fp.display()
                )))
            }
            "insert" => {
                let insert_at = ((cell_index + 1) as usize).min(cells.len());
                let cell = Self::new_cell(new_source, cell_type, generate_id);
                cells.insert(insert_at, cell);
                let serialized = serde_json::to_string_pretty(&nb).unwrap_or_default();
                if let Err(e) = fs::write(&fp, serialized) {
                    return Ok(Value::String(format!("Error editing notebook: {e}")));
                }
                Ok(Value::String(format!(
                    "Successfully inserted cell at index {insert_at} in {}",
                    fp.display()
                )))
            }
            _ => {
                if cell_index < 0 || (cell_index as usize) >= cells.len() {
                    return Ok(Value::String(format!(
                        "Error: cell_index {cell_index} out of range (notebook has {} cells)",
                        cells.len()
                    )));
                }
                let idx = cell_index as usize;
                if let Some(cell) = cells[idx].as_object_mut() {
                    cell.insert("source".into(), Value::String(new_source.into()));
                    let current_type = cell
                        .get("cell_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if current_type != cell_type {
                        cell.insert("cell_type".into(), Value::String(cell_type.into()));
                        if cell_type == "code" {
                            cell.entry("outputs").or_insert_with(|| Value::Array(Vec::new()));
                            cell.entry("execution_count").or_insert(Value::Null);
                        } else {
                            cell.remove("outputs");
                            cell.remove("execution_count");
                        }
                    }
                }
                let serialized = serde_json::to_string_pretty(&nb).unwrap_or_default();
                if let Err(e) = fs::write(&fp, serialized) {
                    return Ok(Value::String(format!("Error editing notebook: {e}")));
                }
                Ok(Value::String(format!(
                    "Successfully edited cell {cell_index} in {}",
                    fp.display()
                )))
            }
        }
    }
}
