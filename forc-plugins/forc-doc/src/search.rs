use crate::doc::{Document, Documentation};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::{collections::HashMap, path::Path};

const JSON_SEARCH_POOL_FILE_NAME: &str = "search_pool.json";
/// Creates the item pool the search bar pulls from.
pub(crate) fn write_search_pool_json(doc_path: &Path, docs: Documentation) -> Result<()> {
    Ok(serde_json::to_writer_pretty(
        fs::File::create(doc_path.join(JSON_SEARCH_POOL_FILE_NAME))?,
        &docs.to_json_value()?,
    )?)
}

impl Documentation {
    /// Generates a mapping of program name to a vector of documentable items within the program
    /// and returns the map as a `serde_json::Value`.
    fn to_json_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        let mut map: HashMap<String, Vec<JsonSearchItem>> = HashMap::with_capacity(self.0.len());
        for doc in self.0.iter() {
            match map.get_mut(doc.module_info.project_name()) {
                Some(items) => {
                    items.push(JsonSearchItem::from(doc));
                }
                None => {
                    map.insert(
                        doc.module_info.project_name().to_string(),
                        vec![JsonSearchItem::from(doc)],
                    );
                }
            }
        }
        serde_json::to_value(map)
    }
}

/// Item information used in the `search_pool.json`.
/// The item name is what the fuzzy search will be
/// matching on, all other information will be used
/// in generating links to the item.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct JsonSearchItem {
    name: String,
    html_filename: String,
    module_info: Vec<String>,
}
impl<'a> From<&'a Document> for JsonSearchItem {
    fn from(value: &'a Document) -> Self {
        Self {
            name: value.item_body.item_name.to_string(),
            html_filename: value.html_filename(),
            module_info: value.module_info.module_prefixes.clone(),
        }
    }
}
