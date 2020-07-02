use super::{ArchetypeDef, WorldMeta};
use crate::{
    query::filter::LayoutFilter,
    storage::{
        archetype::{Archetype, ArchetypeIndex},
        component::ComponentTypeId,
        UnknownComponentStorage,
    },
    world::World,
};
use itertools::Itertools;
use serde::ser::{Serialize, SerializeMap, SerializeStruct, Serializer};
use std::{collections::HashMap, marker::PhantomData};

pub trait WorldSerializer {
    type TypeId: Serialize + Ord;

    fn map_id(&self, type_id: ComponentTypeId) -> Option<Self::TypeId>;
    unsafe fn serialize_component_slice<S: Serializer>(
        &self,
        ty: ComponentTypeId,
        ptr: *const u8,
        count: usize,
        serializer: S,
    ) -> Result<S::Ok, S::Error>;
}

pub struct SerializableWorld<'a, F: LayoutFilter, W: WorldSerializer> {
    world: &'a World,
    filter: F,
    world_serializer: &'a W,
}

impl<'a, F: LayoutFilter, W: WorldSerializer> Serialize for SerializableWorld<'a, F, W> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_world(serializer, self.world, &self.filter, self.world_serializer)
    }
}

pub fn as_serializable<'a, F: LayoutFilter, W: WorldSerializer>(
    world: &'a World,
    filter: F,
    world_serializer: &'a W,
) -> SerializableWorld<'a, F, W> {
    SerializableWorld {
        world,
        filter,
        world_serializer,
    }
}

fn serialize_world<S, F, W>(
    serializer: S,
    world: &World,
    filter: &F,
    world_serializer: &W,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    F: LayoutFilter,
    W: WorldSerializer,
{
    let archetypes = world
        .archetypes()
        .iter()
        .enumerate()
        .filter(|(_, arch)| {
            filter
                .matches_layout(arch.layout().component_types())
                .is_pass()
        })
        .map(|(i, arch)| (ArchetypeIndex(i as u32), arch))
        .collect::<Vec<_>>();

    let type_mappings = archetypes
        .iter()
        .flat_map(|(_, arch)| arch.layout().component_types())
        .unique()
        .filter_map(|id| world_serializer.map_id(*id).map(|mapped| (*id, mapped)))
        .collect::<HashMap<ComponentTypeId, W::TypeId>>();

    let archetype_defs = archetypes
        .iter()
        .map(|(_, archetype)| {
            let components = archetype
                .layout()
                .component_types()
                .iter()
                .filter_map(|type_id| type_mappings.get(type_id))
                .collect::<Vec<_>>();
            ArchetypeDef {
                components,
                entities: archetype.entities().to_vec(),
            }
        })
        .collect::<Vec<_>>();

    let mut root = serializer.serialize_struct("World", 3)?;

    // serialize world metadata
    root.serialize_field(
        "_meta",
        &WorldMeta {
            entity_id_stride: world.entity_allocator().stride(),
            entity_id_offset: world.entity_allocator().offset(),
            entity_id_next: world.entity_allocator().head(),
            component_groups: world
                .groups()
                .iter()
                .filter(|group| group.components().count() > 1)
                .map(|group| {
                    group
                        .components()
                        .filter_map(|type_id| world_serializer.map_id(type_id))
                        .collect()
                })
                .collect(),
        },
    )?;

    // serialize archetypes
    root.serialize_field("archetypes", &archetype_defs)?;

    // serialize components
    root.serialize_field(
        "components",
        &Components {
            world_serializer,
            world,
            type_mappings,
            archetypes,
        },
    )?;

    root.end()
}

struct Components<'a, W: WorldSerializer> {
    world_serializer: &'a W,
    world: &'a World,
    type_mappings: HashMap<ComponentTypeId, W::TypeId>,
    archetypes: Vec<(ArchetypeIndex, &'a Archetype)>,
}

impl<'a, W: WorldSerializer> Serialize for Components<'a, W> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut components = self
            .type_mappings
            .iter()
            .map(|(type_id, mapped)| {
                (
                    mapped,
                    SerializableComponentStorage {
                        storage: self.world.components().get(*type_id).unwrap(),
                        world_serializer: self.world_serializer,
                        type_id: *type_id,
                        archetypes: &self.archetypes,
                    },
                )
            })
            .collect::<Vec<_>>();
        components.sort_by(|a, b| a.0.cmp(&b.0));

        let mut root = serializer.serialize_map(Some(components.len()))?;
        for (mapped, storage) in components {
            root.serialize_entry(mapped, &storage)?;
        }
        root.end()
    }
}

struct SerializableComponentStorage<'a, W: WorldSerializer> {
    storage: &'a dyn UnknownComponentStorage,
    world_serializer: &'a W,
    type_id: ComponentTypeId,
    archetypes: &'a [(ArchetypeIndex, &'a Archetype)],
}

impl<'a, W: WorldSerializer> Serialize for SerializableComponentStorage<'a, W> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let slices = self
            .archetypes
            .iter()
            .enumerate()
            .filter_map(|(local_index, (arch_index, _))| {
                self.storage
                    .get_raw(*arch_index)
                    .map(|(ptr, len)| (local_index, ptr, len))
            })
            .map(|(local_index, ptr, len)| {
                (
                    ArchetypeIndex(local_index as u32),
                    SerializableSlice {
                        ptr,
                        len,
                        type_id: self.type_id,
                        world_serializer: self.world_serializer,
                        _phantom: PhantomData,
                    },
                )
            })
            .collect::<Vec<_>>();

        let mut root = serializer.serialize_map(Some(slices.len()))?;
        for (idx, slice) in slices {
            root.serialize_entry(&idx, &slice)?;
        }
        root.end()
    }
}

struct SerializableSlice<'a, W: WorldSerializer> {
    type_id: ComponentTypeId,
    ptr: *const u8,
    len: usize,
    world_serializer: &'a W,
    _phantom: PhantomData<&'a u8>,
}

impl<'a, W: WorldSerializer> Serialize for SerializableSlice<'a, W> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        unsafe {
            self.world_serializer.serialize_component_slice(
                self.type_id,
                self.ptr,
                self.len,
                serializer,
            )
        }
    }
}
