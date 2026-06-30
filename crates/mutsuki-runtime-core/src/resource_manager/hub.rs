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
struct TypedResourceStore {
    entries: HashMap<String, ResourceEntry>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ResourceHub {
    frozen: TypedResourceStore,
    snapshots: TypedResourceStore,
    facts: TypedResourceStore,
    cow: TypedResourceStore,
    capabilities: TypedResourceStore,
    streams: TypedResourceStore,
    transactions: TypedResourceStore,
}

impl ResourceHub {
    pub(super) fn insert(&mut self, entry: ResourceEntry) {
        self.store_mut(&entry.descriptor.semantic)
            .entries
            .insert(entry.descriptor.ref_id.clone(), entry);
    }

    pub(super) fn get(&self, ref_id: &str) -> Option<&ResourceEntry> {
        self.stores()
            .into_iter()
            .find_map(|store| store.entries.get(ref_id))
    }

    pub(super) fn get_mut(&mut self, ref_id: &str) -> Option<&mut ResourceEntry> {
        for store in self.stores_mut() {
            if store.entries.contains_key(ref_id) {
                return store.entries.get_mut(ref_id);
            }
        }
        None
    }

    pub(super) fn remove(&mut self, ref_id: &str) -> Option<ResourceEntry> {
        for store in self.stores_mut() {
            if let Some(entry) = store.entries.remove(ref_id) {
                return Some(entry);
            }
        }
        None
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.stores()
            .into_iter()
            .flat_map(|store| store.entries.values())
    }

    pub(super) fn store_name(&self, ref_id: &str) -> Option<&'static str> {
        [
            ("frozen", &self.frozen),
            ("snapshots", &self.snapshots),
            ("facts", &self.facts),
            ("cow", &self.cow),
            ("capabilities", &self.capabilities),
            ("streams", &self.streams),
            ("transactions", &self.transactions),
        ]
        .into_iter()
        .find_map(|(name, store)| store.entries.contains_key(ref_id).then_some(name))
    }

    fn stores(&self) -> Vec<&TypedResourceStore> {
        vec![
            &self.frozen,
            &self.snapshots,
            &self.facts,
            &self.cow,
            &self.capabilities,
            &self.streams,
            &self.transactions,
        ]
    }

    fn stores_mut(&mut self) -> Vec<&mut TypedResourceStore> {
        vec![
            &mut self.frozen,
            &mut self.snapshots,
            &mut self.facts,
            &mut self.cow,
            &mut self.capabilities,
            &mut self.streams,
            &mut self.transactions,
        ]
    }

    fn store_mut(&mut self, semantic: &ResourceSemantic) -> &mut TypedResourceStore {
        match semantic {
            ResourceSemantic::FrozenValue => &mut self.frozen,
            ResourceSemantic::VersionedSnapshot => &mut self.snapshots,
            ResourceSemantic::ReadOnlyFact => &mut self.facts,
            ResourceSemantic::CowVersionedState => &mut self.cow,
            ResourceSemantic::CapabilityResource => &mut self.capabilities,
            ResourceSemantic::StreamResource => &mut self.streams,
            ResourceSemantic::TransactionResource => &mut self.transactions,
        }
    }
}
