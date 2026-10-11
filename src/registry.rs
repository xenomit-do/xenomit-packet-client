use crate::generated::schema::{ModuleSchema, SCHEMAS};

/// Look up the schema for a module id (binary search over `SCHEMAS`).
pub fn schema(id: i16) -> Option<&'static ModuleSchema> {
    SCHEMAS
        .binary_search_by_key(&id, |s| s.id)
        .ok()
        .map(|i| &SCHEMAS[i])
}

/// Registered type name for a module id, if known.
pub fn module_name(id: i16) -> Option<&'static str> {
    schema(id).map(|s| s.name)
}

/// Module id for a registered type name (linear scan; use sparingly).
pub fn module_id(name: &str) -> Option<i16> {
    SCHEMAS.iter().find(|s| s.name == name).map(|s| s.id)
}
