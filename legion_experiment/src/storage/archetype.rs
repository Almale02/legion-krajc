use super::{
    component::{Component, ComponentTypeId},
    UnknownComponentStorage,
};
use crate::entity::Entity;
use std::{
    ops::{Index, IndexMut},
    sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ArchetypeIndex(pub u32);

impl Index<ArchetypeIndex> for [Archetype] {
    type Output = Archetype;

    fn index(&self, index: ArchetypeIndex) -> &Self::Output { &self[index.0 as usize] }
}

impl IndexMut<ArchetypeIndex> for [Archetype] {
    fn index_mut(&mut self, index: ArchetypeIndex) -> &mut Self::Output {
        &mut self[index.0 as usize]
    }
}

impl Index<ArchetypeIndex> for Vec<Archetype> {
    type Output = Archetype;

    fn index(&self, index: ArchetypeIndex) -> &Self::Output { &self[index.0 as usize] }
}

impl IndexMut<ArchetypeIndex> for Vec<Archetype> {
    fn index_mut(&mut self, index: ArchetypeIndex) -> &mut Self::Output {
        &mut self[index.0 as usize]
    }
}

#[derive(Debug)]
pub struct Archetype {
    index: ArchetypeIndex,
    entities: Vec<Entity>,
    layout: Arc<EntityLayout>,
}

impl Archetype {
    pub fn new(index: ArchetypeIndex, layout: EntityLayout) -> Self {
        Self {
            index,
            layout: Arc::new(layout),
            entities: Vec::new(),
        }
    }

    pub fn index(&self) -> ArchetypeIndex { self.index }

    pub fn layout(&self) -> &Arc<EntityLayout> { &self.layout }

    pub fn entities(&self) -> &[Entity] { &self.entities }

    pub fn entities_mut(&mut self) -> &mut Vec<Entity> { &mut self.entities }
}

#[derive(Default, Debug, Clone)]
pub struct EntityLayout {
    components: Vec<ComponentTypeId>,
    component_constructors: Vec<fn() -> Box<dyn UnknownComponentStorage>>,
}

impl EntityLayout {
    pub fn new() -> Self { Self::default() }

    pub fn register_component<T: Component>(&mut self) {
        let type_id = ComponentTypeId::of::<T>();
        assert!(
            !self.components.contains(&type_id),
            "only one component of a given type may be attached to a single entity"
        );
        self.components.push(type_id);
        self.component_constructors
            .push(|| Box::new(T::Storage::default()));
    }

    pub unsafe fn register_component_raw(
        &mut self,
        type_id: ComponentTypeId,
        f: fn() -> Box<dyn UnknownComponentStorage>,
    ) {
        assert!(
            !self.components.contains(&type_id),
            "only one component of a given type may be attached to a single entity"
        );
        self.components.push(type_id);
        self.component_constructors.push(f);
    }

    pub fn component_types(&self) -> &[ComponentTypeId] { &self.components }

    pub fn component_constructors(&self) -> &[fn() -> Box<dyn UnknownComponentStorage>] {
        &self.component_constructors
    }

    pub fn has_component<T: Component>(&self) -> bool {
        self.has_component_by_id(ComponentTypeId::of::<T>())
    }

    pub fn has_component_by_id(&self, type_id: ComponentTypeId) -> bool {
        self.components.contains(&type_id)
    }
}
