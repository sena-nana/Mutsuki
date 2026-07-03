use std::collections::HashMap;

use mutsuki_runtime_contracts::{LeaseToken, ResourceRef, ResourceSemantic};

#[derive(Clone, Debug)]
pub(super) struct ResourceEntry {
    pub(super) descriptor: ResourceRef,
    pub(super) writer: Option<LeaseToken>,
}

impl ResourceEntry {
    pub(super) fn new(descriptor: ResourceRef) -> Self {
        Self {
            descriptor,
            writer: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct ResourceHub {
    entries: HashMap<String, ResourceEntry>,
}

impl ResourceHub {
    pub(super) fn insert(&mut self, entry: ResourceEntry) {
        self.entries.insert(entry.descriptor.ref_id.clone(), entry);
    }

    pub(super) fn get(&self, ref_id: &str) -> Option<&ResourceEntry> {
        self.entries.get(ref_id)
    }

    pub(super) fn get_mut(&mut self, ref_id: &str) -> Option<&mut ResourceEntry> {
        self.entries.get_mut(ref_id)
    }

    pub(super) fn remove(&mut self, ref_id: &str) -> Option<ResourceEntry> {
        self.entries.remove(ref_id)
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.entries.values()
    }

    pub(super) fn store_name(&self, ref_id: &str) -> Option<&'static str> {
        self.entries
            .get(ref_id)
            .map(|entry| semantic_store_name(&entry.descriptor.semantic))
    }
}

fn semantic_store_name(semantic: &ResourceSemantic) -> &'static str {
    match semantic {
        ResourceSemantic::FrozenValue => "frozen",
        ResourceSemantic::VersionedSnapshot => "snapshots",
        ResourceSemantic::ReadOnlyFact => "facts",
        ResourceSemantic::CowVersionedState => "cow",
        ResourceSemantic::CapabilityResource => "capabilities",
        ResourceSemantic::StreamResource => "streams",
        ResourceSemantic::TransactionResource => "transactions",
    }
}
