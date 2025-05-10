use std::collections::{HashMap, hash_map::Entry};

use axum::extract::{Path as ExtractPath, Query, State};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    api::response::{ApiResponse, ApiResult},
    database::{FileDatabase, get_metadata::PathMetadataPair, virtual_path::VirtualPath},
};

use super::RequestTimestamp;

#[derive(Debug, Serialize, ToSchema)]
pub struct SerializableMetadata {
    original_file_name: String,
    size_in_bytes: usize,
    hash: String,
    upload_timestamp: String,
}

#[derive(Debug, ToSchema)]
enum FileNode {
    File(SerializableMetadata),
    Dir(HashMap<String, FileNode>),
}

impl Serialize for FileNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            FileNode::File(metadata) => metadata.serialize(serializer),
            FileNode::Dir(nodes) => nodes.serialize(serializer),
        }
    }
}

#[derive(Debug, ToSchema)]
pub struct FileTree {
    root: HashMap<String, FileNode>,
}

impl FileTree {
    fn new() -> Self {
        Self {
            root: HashMap::new(),
        }
    }

    fn insert_file(&mut self, entry: PathMetadataPair, prefix: &str) {
        let components = entry.virtual_path.path()[prefix.len()..]
            .split(VirtualPath::SEPARATOR)
            .collect::<Vec<_>>();

        let mut current = &mut self.root;

        for (index, component) in components.iter().enumerate() {
            current = match current.entry((*component).to_owned()) {
                Entry::Vacant(e) => {
                    if index == components.len() - 1 {
                        e.insert(FileNode::File(SerializableMetadata {
                            original_file_name: entry.metadata.original_file_name,
                            size_in_bytes: entry.metadata.size_in_bytes,
                            hash: entry.metadata.hash,
                            upload_timestamp: entry.upload_timestamp.to_rfc3339(),
                        }));
                        return;
                    } else {
                        match e.insert(FileNode::Dir(HashMap::new())) {
                            FileNode::Dir(m) => m,
                            FileNode::File(_) => unreachable!(),
                        }
                    }
                }
                Entry::Occupied(e) => match e.into_mut() {
                    FileNode::Dir(m) => m,
                    FileNode::File(_) => unreachable!(),
                },
            }
        }
    }
}

impl Serialize for FileTree {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut root_node = HashMap::new();
        root_node.insert("files", &self.root);
        root_node.serialize(serializer)
    }
}

#[utoipa::path(
    get,
    path = "/tree/{*path}",
    tag = "files",
    responses(
        (status = 200, description = "The filepaths were successfully returned")
    )
)]
pub async fn endpoint_tree(
    State(database): State<FileDatabase>,
    path: Option<ExtractPath<String>>,
    Query(timestamp): Query<RequestTimestamp>,
) -> ApiResult<FileTree> {
    let virtual_path = match path {
        Some(path) => VirtualPath::from(path.0),
        None => VirtualPath::default(),
    };

    let entries = database
        .get_tree_metadata(virtual_path.clone(), timestamp.try_into()?)
        .await?;

    let mut tree = FileTree::new();

    for entry in entries {
        tree.insert_file(entry, virtual_path.path());
    }

    Ok(ApiResponse::success(tree))
}

#[utoipa::path(
    get,
    path = "/metadata/{*path}",
    tag = "files",
    responses(
        (status = 200, description = "The metadata were successfully returned")
    )
)]
pub async fn endpoint_metadata(
    State(database): State<FileDatabase>,
    path: ExtractPath<String>,
    Query(timestamp): Query<RequestTimestamp>,
) -> ApiResult<SerializableMetadata> {
    let virtual_path = VirtualPath::from(path.0);
    let metadata = database
        .get_file_metadata(virtual_path, timestamp.try_into()?)
        .await?;

    Ok(ApiResponse::success(SerializableMetadata {
        original_file_name: metadata.metadata.original_file_name,
        size_in_bytes: metadata.metadata.size_in_bytes,
        hash: metadata.metadata.hash,
        upload_timestamp: metadata.upload_timestamp.to_rfc3339(),
    }))
}
