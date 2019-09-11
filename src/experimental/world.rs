use crate::experimental::borrow::Exclusive;
use crate::experimental::borrow::Ref;
use crate::experimental::borrow::RefMut;
use crate::experimental::borrow::Shared;
use crate::experimental::entity::BlockAllocator;
use crate::experimental::entity::Entity;
use crate::experimental::entity::EntityAllocator;
use crate::experimental::entity::EntityLocation;
use crate::experimental::filter::ArchetypeFilterData;
use crate::experimental::filter::ChunkFilterData;
use crate::experimental::filter::Filter;
use crate::experimental::filter::FilterResult;
use crate::experimental::storage::ArchetypeData;
use crate::experimental::storage::ArchetypeDescription;
use crate::experimental::storage::Component;
use crate::experimental::storage::ComponentMeta;
use crate::experimental::storage::ComponentStorage;
use crate::experimental::storage::ComponentTypeId;
use crate::experimental::storage::SliceVecIter;
use crate::experimental::storage::Storage;
use crate::experimental::storage::Tag;
use crate::experimental::storage::TagMeta;
use crate::experimental::storage::TagTypeId;
use crate::experimental::storage::Tags;
use parking_lot::Mutex;
use std::cell::UnsafeCell;
use std::iter::Enumerate;
use std::iter::Peekable;
use std::iter::Repeat;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::Arc;

/// The `Universe` is a factory for creating `World`s.
///
/// Entities inserted into worlds created within the same universe are guarenteed to have
/// unique `Entity` IDs, even across worlds.
#[derive(Debug)]
pub struct Universe {
    allocator: Arc<Mutex<BlockAllocator>>,
}

impl Universe {
    /// Creates a new `Universe`.
    pub fn new() -> Self { Self::default() }

    /// Creates a new `World` within this `Unvierse`.
    ///
    /// Entities inserted into worlds created within the same universe are guarenteed to have
    /// unique `Entity` IDs, even across worlds.
    pub fn create_world(&self) -> World { World::new(EntityAllocator::new(self.allocator.clone())) }
}

impl Default for Universe {
    fn default() -> Self {
        Self {
            allocator: Arc::new(Mutex::new(BlockAllocator::new())),
        }
    }
}

/// Contains queryable collections of data associated with `Entity`s.
pub struct World {
    archetypes: UnsafeCell<Storage>,
    entity_allocator: EntityAllocator,
}

impl World {
    fn new(allocator: EntityAllocator) -> Self {
        Self {
            archetypes: UnsafeCell::new(Storage::default()),
            entity_allocator: allocator,
        }
    }

    pub(crate) fn storage(&self) -> &Storage { unsafe { &*self.archetypes.get() } }

    pub(crate) fn storage_mut(&mut self) -> &mut Storage { unsafe { &mut *self.archetypes.get() } }

    /// Inserts new entities into the world.
    ///
    /// # Examples
    ///
    /// Inserting entity tuples:
    ///
    /// ```
    /// # use legion::experimental::prelude::*;
    /// # #[derive(Copy, Clone, Debug, PartialEq)]
    /// # struct Position(f32);
    /// # #[derive(Copy, Clone, Debug, PartialEq)]
    /// # struct Rotation(f32);
    /// # let universe = Universe::new();
    /// # let mut world = universe.create_world();
    /// # let model = 0u8;
    /// # let color = 0u16;
    /// let tags = (model, color);
    /// let data = vec![
    ///     (Position(0.0), Rotation(0.0)),
    ///     (Position(1.0), Rotation(1.0)),
    ///     (Position(2.0), Rotation(2.0)),
    /// ];
    /// world.insert(tags, data);
    /// ```
    pub fn insert<T, C>(&mut self, mut tags: T, components: C) -> &[Entity]
    where
        T: TagSet + TagLayout + for<'a> Filter<ChunkFilterData<'a>>,
        C: IntoComponentSource,
    {
        // find or create archetype
        let mut components = components.into();
        let archetype_id = self.find_or_create_archetype(&mut tags, &mut components);

        self.entity_allocator.clear_allocation_buffer();

        // insert components into chunks
        while !components.is_empty() {
            // find or create chunk
            let chunk_id = self.find_or_create_chunk(archetype_id, &mut tags);

            // get chunk component storage
            let chunks = unsafe { (&mut *self.archetypes.get()).data_unchecked_mut(archetype_id) };
            let component_storage = chunks.component_chunk_mut(chunk_id).unwrap();

            // insert as many components as we can into the chunk
            let allocated = components.write(&mut self.entity_allocator, component_storage);

            // record new entity locations
            let start = component_storage.len() - allocated;
            let added = component_storage.entities().iter().enumerate().skip(start);
            for (i, e) in added {
                let location = EntityLocation::new(archetype_id, chunk_id, i);
                self.entity_allocator.set_location(e.index(), location);
            }
        }

        self.entity_allocator.allocation_buffer()
    }

    /// Removes the given `Entity` from the `World`.
    ///
    /// Returns `true` if the entity was deleted; else `false`.
    pub fn delete(&mut self, entity: Entity) -> bool {
        if let Some(location) = self.entity_allocator.delete_entity(entity) {
            // find entity's chunk
            let chunks = self.storage_mut().data_mut(location.archetype()).unwrap();
            let chunk = chunks.component_chunk_mut(location.chunk()).unwrap();

            // swap remove with last entity in chunk
            if let Some(swapped) = chunk.swap_remove(location.component(), true) {
                // record swapped entity's new location
                self.entity_allocator
                    .set_location(swapped.index(), location);
            }

            true
        } else {
            false
        }
    }

    fn find_chunk_delta(
        &mut self,
        source_location: EntityLocation,
        add_components: &[(ComponentTypeId, ComponentMeta)],
        remove_components: &[ComponentTypeId],
        add_tags: &[(TagTypeId, TagMeta, *const u8)],
        remove_tags: &[TagTypeId],
    ) -> (usize, usize) {
        let archetype = {
            let result = {
                let source_archetype = self.storage().data(source_location.archetype()).unwrap();

                // find target chunk
                let mut component_layout = DynamicComponentLayout {
                    existing: source_archetype.description().components(),
                    add: add_components,
                    remove: remove_components,
                };

                let mut tag_layout = DynamicTagLayout {
                    storage: self.storage(),
                    archetype: source_location.archetype(),
                    chunk: source_location.chunk(),
                    existing: source_archetype.description().tags(),
                    add: add_tags,
                    remove: remove_tags,
                };

                let archetype = self.find_archetype(&mut tag_layout, &mut component_layout);
                if let Some(archetype) = archetype.as_ref() {
                    if let Some(chunk) = self.find_chunk(*archetype, &mut tag_layout) {
                        // fast path: chunk already exists
                        return (*archetype, chunk);
                    }

                    Ok(*archetype)
                } else {
                    let mut description = ArchetypeDescription::default();
                    component_layout.tailor_archetype(&mut description);
                    tag_layout.tailor_archetype(&mut description);

                    Err(description)
                }
            };

            match result {
                Ok(arch) => arch,
                Err(desc) => {
                    let (index, _) = self.storage_mut().alloc_archetype(desc);
                    index
                }
            }
        };

        // slow path: create new chunk
        let source_archetype = self.storage().data(source_location.archetype()).unwrap();
        let mut tags = source_archetype.tags().tag_set(source_location.chunk());
        for type_id in remove_tags.iter() {
            tags.remove(*type_id);
        }
        for (type_id, meta, ptr) in add_tags.iter() {
            tags.push(*type_id, *meta, *ptr);
        }

        let chunk = self.create_chunk(archetype, &tags);

        (archetype, chunk)
    }

    fn move_entity(
        &mut self,
        entity: Entity,
        add_components: &[(ComponentTypeId, ComponentMeta)],
        remove_components: &[ComponentTypeId],
        add_tags: &[(TagTypeId, TagMeta, *const u8)],
        remove_tags: &[TagTypeId],
    ) -> &mut ComponentStorage {
        let location = self
            .entity_allocator
            .get_location(entity.index())
            .expect("entity not found");

        // find or create the target chunk
        let (target_arch_index, target_chunk_index) = self.find_chunk_delta(
            location,
            add_components,
            remove_components,
            add_tags,
            remove_tags,
        );

        // Safety Note:
        // It is only safe for us to have 2 &mut references to storage here because
        // we know we are only going to be modifying two chunks that are at different
        // indexes.

        // fetch entity's chunk
        let chunks = unsafe { &mut *self.archetypes.get() }
            .data_mut(location.archetype())
            .unwrap();
        let current_chunk = chunks.component_chunk_mut(location.chunk()).unwrap();

        // fetch target chunk
        let chunks = unsafe { &mut *self.archetypes.get() }
            .data_mut(target_arch_index)
            .unwrap();
        let target_chunk = chunks.component_chunk_mut(target_chunk_index).unwrap();

        // move existing data over into new chunk
        if let Some(swapped) = current_chunk.move_entity(target_chunk, location.component()) {
            // update location of any entity that was moved into the previous location
            self.entity_allocator
                .set_location(swapped.index(), location);
        }

        // record the entity's new location
        self.entity_allocator.set_location(
            entity.index(),
            EntityLocation::new(
                target_arch_index,
                target_chunk_index,
                target_chunk.len() - 1,
            ),
        );

        target_chunk
    }

    /// Adds a component to an entity, or set's its value if the component is
    /// already present.
    pub fn add_component<T: Component>(&mut self, entity: Entity, component: T) {
        if let Some(mut comp) = self.get_component_mut(entity) {
            *comp = component;
            return;
        }

        // move the entity into a suitable chunk
        let target_chunk = self.move_entity(
            entity,
            &[(ComponentTypeId::of::<T>(), ComponentMeta::of::<T>())],
            &[],
            &[],
            &[],
        );

        // push new component into chunk
        let (_, components) = target_chunk.write();
        unsafe {
            let components = &mut *components.get();
            components
                .get_mut(ComponentTypeId::of::<T>())
                .unwrap()
                .writer()
                .push(&[component]);
        }
    }

    /// Removes a component from an entity.
    pub fn remove_component<T: Component>(&mut self, entity: Entity) {
        if self.get_component::<T>(entity).is_some() {
            // move the entity into a suitable chunk
            self.move_entity(entity, &[], &[ComponentTypeId::of::<T>()], &[], &[]);
        }
    }

    /// Adds a tag to an entity, or set's its value if the tag is
    /// already present.
    pub fn add_tag<T: Tag>(&mut self, entity: Entity, tag: T) {
        if self.get_tag::<T>(entity).is_some() {
            self.remove_tag::<T>(entity);
        }

        // move the entity into a suitable chunk
        self.move_entity(
            entity,
            &[],
            &[],
            &[(
                TagTypeId::of::<T>(),
                TagMeta::of::<T>(),
                &tag as *const _ as *const u8,
            )],
            &[],
        );
    }

    /// Removes a tag from an entity.
    pub fn remove_tag<T: Tag>(&mut self, entity: Entity) {
        if self.get_tag::<T>(entity).is_some() {
            // move the entity into a suitable chunk
            self.move_entity(entity, &[], &[], &[], &[TagTypeId::of::<T>()]);
        }
    }

    /// Borrows component data for the given entity.
    ///
    /// Returns `Some(data)` if the entity was found and contains the specified data.
    /// Otherwise `None` is returned.
    ///
    /// # Panics
    ///
    /// This function borrows all components of type `T` in the world. It may panic if
    /// any other code is currently borrowing `T` mutably (such as in a query).
    pub fn get_component<T: Component>(&self, entity: Entity) -> Option<Ref<Shared, T>> {
        if !self.is_alive(entity) {
            return None;
        }

        let location = self.entity_allocator.get_location(entity.index())?;
        let chunks = self.storage().data(location.archetype())?;
        let chunk = chunks.component_chunk(location.chunk())?;
        let (slice_borrow, slice) = unsafe {
            chunk
                .components(ComponentTypeId::of::<T>())?
                .data_slice::<T>()
                .deconstruct()
        };
        let component = slice.get(location.component())?;

        Some(Ref::new(slice_borrow, component))
    }

    /// Mutably borrows entity data for the given entity.
    ///
    /// Returns `Some(data)` if the entity was found and contains the specified data.
    /// Otherwise `None` is returned.
    ///
    /// # Panics
    ///
    /// This function borrows all components of type `T` in the world. It may panic if
    /// any other code is currently borrowing `T` (such as in a query).
    pub fn get_component_mut<T: Component>(&self, entity: Entity) -> Option<RefMut<Exclusive, T>> {
        if !self.is_alive(entity) {
            return None;
        }

        let location = self.entity_allocator.get_location(entity.index())?;
        let chunks = self.storage().data(location.archetype())?;
        let chunk = chunks.component_chunk(location.chunk())?;
        let (slice_borrow, slice) = unsafe {
            chunk
                .components(ComponentTypeId::of::<T>())?
                .data_slice_mut::<T>()
                .deconstruct()
        };
        let component = slice.get_mut(location.component())?;

        Some(RefMut::new(slice_borrow, component))
    }

    /// Gets tag data for the given entity.
    ///
    /// Returns `Some(data)` if the entity was found and contains the specified data.
    /// Otherwise `None` is returned.
    pub fn get_tag<T: Tag>(&self, entity: Entity) -> Option<&T> {
        if !self.is_alive(entity) {
            return None;
        }

        let location = self.entity_allocator.get_location(entity.index())?;
        let chunks = self.storage().data(location.archetype())?;
        let tags = chunks.tags().get(TagTypeId::of::<T>())?;
        unsafe { tags.data_slice::<T>().get(location.chunk()) }
    }

    /// Determines if the given `Entity` is alive within this `World`.
    pub fn is_alive(&self, entity: Entity) -> bool { self.entity_allocator.is_alive(entity) }

    fn find_archetype<T, C>(&self, tags: &mut T, components: &mut C) -> Option<usize>
    where
        T: TagLayout,
        C: ComponentLayout,
    {
        // search for an archetype with an exact match for the desired component layout
        let archetype_data = ArchetypeFilterData {
            component_types: self.storage().component_types(),
            tag_types: self.storage().tag_types(),
        };

        let tag_iter = tags.collect(archetype_data);
        let tag_matches =
            tag_iter.map(|x| <T as Filter<ArchetypeFilterData<'_>>>::is_match(tags, &x));
        let component_iter = components.collect(archetype_data);
        let component_matches = component_iter.map(|x| components.is_match(&x));
        tag_matches
            .zip(component_matches)
            .enumerate()
            .filter(|(_, (t, c))| t.is_pass() && c.is_pass())
            .map(|(i, _)| i)
            .next()
    }

    fn create_archetype<T, C>(&mut self, tags: &T, components: &C) -> usize
    where
        T: TagLayout,
        C: ComponentLayout,
    {
        let mut description = ArchetypeDescription::default();
        tags.tailor_archetype(&mut description);
        components.tailor_archetype(&mut description);

        let (index, _) = self.storage_mut().alloc_archetype(description);
        index
    }

    fn find_or_create_archetype<T, C>(&mut self, tags: &mut T, components: &mut C) -> usize
    where
        T: TagLayout,
        C: ComponentLayout,
    {
        if let Some(i) = self.find_archetype(tags, components) {
            i
        } else {
            self.create_archetype(tags, components)
        }
    }

    fn find_chunk<T>(&self, archetype: usize, tags: &mut T) -> Option<usize>
    where
        T: for<'a> Filter<ChunkFilterData<'a>>,
    {
        // fetch the archetype, we can already assume that the archetype index is valid
        let archetype_data = unsafe { self.storage().data_unchecked(archetype) };

        // find a chunk with the correct tags
        let chunk_filter_data = ChunkFilterData {
            archetype_data: archetype_data.deref(),
        };

        let chunk_iter = tags.collect(chunk_filter_data);
        if let Some(i) = chunk_iter
            .map(|x| <T as Filter<ChunkFilterData<'_>>>::is_match(tags, &x))
            .zip(archetype_data.iter_component_chunks())
            .enumerate()
            .filter(|(_, (matches, components))| matches.is_pass() && !components.is_full())
            .map(|(i, _)| i)
            .next()
        {
            return Some(i);
        }

        None
    }

    fn create_chunk<T>(&mut self, archetype: usize, tags: &T) -> usize
    where
        T: TagSet,
    {
        let archetype_data = unsafe { self.storage_mut().data_unchecked_mut(archetype) };
        let (id, chunk_tags, _) = archetype_data.alloc_chunk();
        tags.write_tags(chunk_tags);
        id
    }

    fn find_or_create_chunk<T>(&mut self, archetype: usize, tags: &mut T) -> usize
    where
        T: TagSet + for<'a> Filter<ChunkFilterData<'a>>,
    {
        if let Some(i) = self.find_chunk(archetype, tags) {
            i
        } else {
            self.create_chunk(archetype, tags)
        }
    }
}

/// Describes the types of a set of components attached to an entity.
pub trait ComponentLayout: Sized + for<'a> Filter<ArchetypeFilterData<'a>> {
    /// Modifies an archetype description to include the components described by this layout.
    fn tailor_archetype(&self, archetype: &mut ArchetypeDescription);
}

/// Describes the types of a set of tags attached to an entity.
pub trait TagLayout: Sized + for<'a> Filter<ArchetypeFilterData<'a>> {
    /// Modifies an archetype description to include the tags described by this layout.
    fn tailor_archetype(&self, archetype: &mut ArchetypeDescription);
}

/// A set of tag values to be attached to an entity.
pub trait TagSet {
    /// Writes the tags in this set to a new chunk.
    fn write_tags(&self, tags: &mut Tags);
}

/// A set of components to be attached to one or more entities.
pub trait ComponentSource: ComponentLayout {
    /// Determines if this component source has any more entity data to write.
    fn is_empty(&mut self) -> bool;

    /// Writes as many components as possible into a chunk.
    fn write(&mut self, allocator: &mut EntityAllocator, chunk: &mut ComponentStorage) -> usize;
}

/// An object that can be converted into a `ComponentSource`.
pub trait IntoComponentSource {
    /// The component source type that can be converted into.
    type Source: ComponentSource;

    /// Converts `self` into a component source.
    fn into(self) -> Self::Source;
}

/// A `ComponentSource` which can insert tuples of components representing each entity into a world.
pub struct ComponentTupleSet<T, I>
where
    I: Iterator<Item = T>,
{
    iter: Peekable<I>,
}

impl<T, I> From<I> for ComponentTupleSet<T, I>
where
    I: Iterator<Item = T>,
    ComponentTupleSet<T, I>: ComponentSource,
{
    fn from(iter: I) -> Self {
        ComponentTupleSet {
            iter: iter.peekable(),
        }
    }
}

impl<I> IntoComponentSource for I
where
    I: IntoIterator,
    ComponentTupleSet<I::Item, I::IntoIter>: ComponentSource,
{
    type Source = ComponentTupleSet<I::Item, I::IntoIter>;

    fn into(self) -> Self::Source {
        ComponentTupleSet {
            iter: self.into_iter().peekable(),
        }
    }
}

mod tuple_impls {
    use super::*;
    use crate::experimental::storage::Component;
    use crate::experimental::storage::ComponentTypeId;
    use crate::experimental::storage::SliceVecIter;
    use crate::experimental::storage::Tag;
    use itertools::Zip;
    use std::iter::Repeat;
    use std::iter::Take;
    use std::slice::Iter;

    macro_rules! impl_data_tuple {
        ( $( $ty: ident => $id: ident ),* ) => {
            impl_data_tuple!(@TAG_SET $( $ty => $id ),*);
            impl_data_tuple!(@COMPONENT_SOURCE $( $ty => $id ),*);
        };
        ( @COMPONENT_SOURCE $( $ty: ident => $id: ident ),* ) => {
            impl<I, $( $ty ),*> ComponentLayout for ComponentTupleSet<($( $ty, )*), I>
            where
                I: Iterator<Item = ($( $ty, )*)> + Sync + Send,
                $( $ty: Component ),*
            {
                fn tailor_archetype(&self, archetype: &mut ArchetypeDescription) {
                    #![allow(unused_variables)]
                    $(
                        archetype.register_component::<$ty>();
                    )*
                }
            }

            impl<I, $( $ty ),*> ComponentSource for ComponentTupleSet<($( $ty, )*), I>
            where
                I: Iterator<Item = ($( $ty, )*)> + Sync + Send,
                $( $ty: Component ),*
            {
                fn is_empty(&mut self) -> bool {
                    self.iter.peek().is_none()
                }

                fn write(&mut self, allocator: &mut EntityAllocator, chunk: &mut ComponentStorage) -> usize {
                    #![allow(unused_variables)]
                    #![allow(unused_unsafe)]
                    #![allow(non_snake_case)]
                    let space = chunk.capacity() - chunk.len();
                    let (entities, components) = chunk.write();
                    let mut count = 0;

                    unsafe {
                        $(
                            let mut $ty = (&mut *components.get()).get_mut(ComponentTypeId::of::<$ty>()).unwrap().writer();
                        )*

                        while let Some(($( $id, )*)) = { if count == space { None } else { self.iter.next() } } {
                            let entity = allocator.create_entity();
                            entities.push(entity);
                            $(
                                let slice = [$id];
                                $ty.push(&slice);
                                std::mem::forget(slice);
                            )*
                            count += 1;
                        }
                    }

                    count
                }
            }

            impl<'a, I, $( $ty ),*> Filter<ArchetypeFilterData<'a>> for ComponentTupleSet<($( $ty, )*), I>
            where
                I: Iterator<Item = ($( $ty, )*)> + Sync + Send,
                $( $ty: Component ),*
            {
                type Iter = SliceVecIter<'a, ComponentTypeId>;

                fn collect(&self, source: ArchetypeFilterData<'a>) -> Self::Iter {
                    source.component_types.iter()
                }

                fn is_match(&mut self, item: &<Self::Iter as Iterator>::Item) -> Option<bool> {
                    let types = &[$( ComponentTypeId::of::<$ty>() ),*];
                    Some(types.len() == item.len() && types.iter().all(|t| item.contains(t)))
                }
            }
        };
        ( @TAG_SET $( $ty: ident => $id: ident ),* ) => {
            impl_data_tuple!(@CHUNK_FILTER $( $ty => $id ),*);

            impl<$( $ty ),*> TagSet for ($( $ty, )*)
            where
                $( $ty: Tag ),*
            {
                fn write_tags(&self, tags: &mut Tags) {
                    #![allow(unused_variables)]
                    #![allow(non_snake_case)]
                    let ($($id,)*) = self;
                    $(
                        unsafe {
                            tags.get_mut(TagTypeId::of::<$ty>())
                                .unwrap()
                                .push($id.clone())
                        };
                    )*
                }
            }

            impl <$( $ty ),*> TagLayout for ($( $ty, )*)
            where
                $( $ty: Tag ),*
            {
                fn tailor_archetype(&self, archetype: &mut ArchetypeDescription) {
                    #![allow(unused_variables)]
                    $(
                        archetype.register_tag::<$ty>();
                    )*
                }
            }

            impl<'a, $( $ty ),*> Filter<ArchetypeFilterData<'a>> for ($( $ty, )*)
            where
                $( $ty: Tag ),*
            {
                type Iter = SliceVecIter<'a, TagTypeId>;

                fn collect(&self, source: ArchetypeFilterData<'a>) -> Self::Iter {
                    source.tag_types.iter()
                }

                fn is_match(&mut self, item: &<Self::Iter as Iterator>::Item) -> Option<bool> {
                    let types = &[$( TagTypeId::of::<$ty>() ),*];
                    Some(types.len() == item.len() && types.iter().all(|t| item.contains(t)))
                }
            }
        };
        ( @CHUNK_FILTER $( $ty: ident => $id: ident ),+ ) => {
            impl<'a, $( $ty ),*> Filter<ChunkFilterData<'a>> for ($( $ty, )*)
            where
                $( $ty: Tag ),*
            {
                type Iter = Zip<($( Iter<'a, $ty>, )*)>;

                fn collect(&self, source: ChunkFilterData<'a>) -> Self::Iter {
                    let iters = (
                        $(
                            unsafe {
                                source.archetype_data
                                    .tags()
                                    .get(TagTypeId::of::<$ty>())
                                    .unwrap()
                                    .data_slice::<$ty>()
                                    .iter()
                            },
                        )*

                    );

                    itertools::multizip(iters)
                }

                fn is_match(&mut self, item: &<Self::Iter as Iterator>::Item) -> Option<bool> {
                    #![allow(non_snake_case)]
                    let ($( $ty, )*) = self;
                    Some(($( &*$ty, )*) == *item)
                }
            }
        };
        ( @CHUNK_FILTER ) => {
            impl<'a> Filter<ChunkFilterData<'a>> for () {
                type Iter = Take<Repeat<()>>;

                fn collect(&self, source: ChunkFilterData<'a>) -> Self::Iter {
                    std::iter::repeat(()).take(source.archetype_data.len())
                }

                fn is_match(&mut self, _: &<Self::Iter as Iterator>::Item) -> Option<bool> {
                    Some(true)
                }
            }
        };
    }

    impl_data_tuple!();
    impl_data_tuple!(A => a);
    impl_data_tuple!(A => a, B => b);
    impl_data_tuple!(A => a, B => b, C => c);
    impl_data_tuple!(A => a, B => b, C => c, D => d);
    impl_data_tuple!(A => a, B => b, C => c, D => d, E => e);
}

struct DynamicComponentLayout<'a> {
    existing: &'a [(ComponentTypeId, ComponentMeta)],
    add: &'a [(ComponentTypeId, ComponentMeta)],
    remove: &'a [ComponentTypeId],
}

impl<'a> ComponentLayout for DynamicComponentLayout<'a> {
    fn tailor_archetype(&self, archetype: &mut ArchetypeDescription) {
        // copy components from existing archetype into new
        // except for those in `remove`
        let components = self
            .existing
            .iter()
            .filter(|(t, _)| !self.remove.contains(t));

        for (comp_type, meta) in components {
            archetype.register_component_raw(*comp_type, *meta);
        }

        // append components from `add`
        for (comp_type, meta) in self.add.iter() {
            archetype.register_component_raw(*comp_type, *meta);
        }
    }
}

impl<'a, 'b> Filter<ArchetypeFilterData<'b>> for DynamicComponentLayout<'a> {
    type Iter = SliceVecIter<'b, ComponentTypeId>;

    fn collect(&self, source: ArchetypeFilterData<'b>) -> Self::Iter {
        source.component_types.iter()
    }

    fn is_match(&mut self, item: &<Self::Iter as Iterator>::Item) -> Option<bool> {
        Some(
            item.len() == (self.existing.len() + self.add.len() - self.remove.len())
                && item.iter().all(|t| {
                    // all types are not in remove
                    !self.remove.contains(t)
                    // any are either in existing or add
                        && (self.existing.iter().any(|(x, _)| x == t)
                            || self.add.iter().any(|(x, _)| x == t))
                }),
        )
    }
}

struct DynamicTagLayout<'a> {
    storage: &'a Storage,
    archetype: usize,
    chunk: usize,
    existing: &'a [(TagTypeId, TagMeta)],
    add: &'a [(TagTypeId, TagMeta, *const u8)],
    remove: &'a [TagTypeId],
}

unsafe impl<'a> Send for DynamicTagLayout<'a> {}

unsafe impl<'a> Sync for DynamicTagLayout<'a> {}

impl<'a> TagLayout for DynamicTagLayout<'a> {
    fn tailor_archetype(&self, archetype: &mut ArchetypeDescription) {
        // copy tags from existing archetype into new
        // except for those in `remove`
        let tags = self
            .existing
            .iter()
            .filter(|(t, _)| !self.remove.contains(t));

        for (tag_type, meta) in tags {
            archetype.register_tag_raw(*tag_type, *meta);
        }

        // append tag from `add`
        for (tag_type, meta, _) in self.add.iter() {
            archetype.register_tag_raw(*tag_type, *meta);
        }
    }
}

impl<'a, 'b> Filter<ArchetypeFilterData<'b>> for DynamicTagLayout<'a> {
    type Iter = SliceVecIter<'b, TagTypeId>;

    fn collect(&self, source: ArchetypeFilterData<'b>) -> Self::Iter { source.tag_types.iter() }

    fn is_match(&mut self, item: &<Self::Iter as Iterator>::Item) -> Option<bool> {
        Some(
            item.len() == (self.existing.len() + self.add.len() - self.remove.len())
                && item.iter().all(|t| {
                    // all types are not in remove
                    !self.remove.contains(t)
                    // any are either in existing or add
                        && (self.existing.iter().any(|(x, _)| x == t)
                            || self.add.iter().any(|(x, _, _)| x == t))
                }),
        )
    }
}

impl<'a, 'b> Filter<ChunkFilterData<'b>> for DynamicTagLayout<'a> {
    type Iter = Enumerate<Repeat<&'b ArchetypeData>>;

    fn collect(&self, source: ChunkFilterData<'b>) -> Self::Iter {
        std::iter::repeat(source.archetype_data).enumerate()
    }

    fn is_match(&mut self, (chunk_index, arch): &<Self::Iter as Iterator>::Item) -> Option<bool> {
        for (type_id, meta) in self.existing {
            if self.remove.contains(type_id) {
                continue;
            }

            unsafe {
                // find the value of the tag in the source chunk
                let (slice_ptr, element_size, _) = self
                    .storage
                    .data(self.archetype)
                    .unwrap()
                    .tags()
                    .get(*type_id)
                    .unwrap()
                    .data_raw();
                let current = slice_ptr.as_ptr().add(self.chunk * element_size);

                // find the value of the tag in the candidate chunk
                let (slice_ptr, element_size, _) = arch.tags().get(*type_id).unwrap().data_raw();
                let candidate = slice_ptr.as_ptr().add(chunk_index * element_size);

                if !meta.equals(current, candidate) {
                    return Some(false);
                }
            }
        }

        for (type_id, meta, ptr) in self.add {
            unsafe {
                let (slice_ptr, element_size, _) = arch.tags().get(*type_id).unwrap().data_raw();
                let candidate = slice_ptr.as_ptr().add(chunk_index * element_size);

                if !meta.equals(*ptr, candidate) {
                    return Some(false);
                }
            }
        }

        Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Pos(f32, f32, f32);
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Rot(f32, f32, f32);
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Scale(f32, f32, f32);
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Vel(f32, f32, f32);
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Accel(f32, f32, f32);
    #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
    struct Model(u32);
    #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
    struct Static;

    fn create() -> World {
        let universe = Universe::new();
        universe.create_world()
    }

    #[test]
    fn create_universe() { Universe::default(); }

    #[test]
    fn create_world() {
        let universe = Universe::new();
        universe.create_world();
    }

    #[test]
    fn insert() {
        let mut world = create();

        let shared = (1usize, 2f32, 3u16);
        let components = vec![(4f32, 5u64, 6u16), (4f32, 5u64, 6u16)];
        world.insert(shared, components);

        assert_eq!(2, world.entity_allocator.allocation_buffer().len());
    }

    #[test]
    fn get_component() {
        let mut world = create();

        let shared = (Static, Model(5));
        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        world.insert(shared, components.clone());

        for (i, e) in world
            .entity_allocator
            .allocation_buffer()
            .to_vec()
            .iter()
            .enumerate()
        {
            match world.get_component(*e) {
                Some(x) => assert_eq!(components.get(i).map(|(x, _)| x), Some(&x as &Pos)),
                None => assert_eq!(components.get(i).map(|(x, _)| x), None),
            }
            match world.get_component(*e) {
                Some(x) => assert_eq!(components.get(i).map(|(_, x)| x), Some(&x as &Rot)),
                None => assert_eq!(components.get(i).map(|(_, x)| x), None),
            }
        }
    }

    #[test]
    fn get_component_wrong_type() {
        let mut world = create();

        world.insert((), vec![(0f64,)]);

        let entity = *world.entity_allocator.allocation_buffer().get(0).unwrap();

        assert!(world.get_component::<i32>(entity).is_none());
    }

    #[test]
    fn get_tag() {
        let mut world = create();

        let shared = (Static, Model(5));
        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        world.insert(shared, components);

        for e in world.entity_allocator.allocation_buffer().to_vec().iter() {
            assert_eq!(&Static, world.get_tag::<Static>(*e).unwrap().deref());
            assert_eq!(&Model(5), world.get_tag::<Model>(*e).unwrap().deref());
        }
    }

    #[test]
    fn get_tag_wrong_type() {
        let mut world = create();

        world.insert((Static,), vec![(0f64,)]);

        let entity = *world.entity_allocator.allocation_buffer().get(0).unwrap();

        assert!(world.get_tag::<Model>(entity).is_none());
    }

    #[test]
    fn delete() {
        let mut world = create();

        let shared = (Static, Model(5));
        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        let entities = world.insert(shared, components).to_vec();

        for e in entities.iter() {
            assert!(world.get_component::<Pos>(*e).is_some());
        }

        for e in entities.iter() {
            world.delete(*e);
            assert!(world.get_component::<Pos>(*e).is_none());
        }
    }

    #[test]
    fn delete_last() {
        let mut world = create();

        let shared = (Static, Model(5));
        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        let entities = world.insert(shared, components.clone()).to_vec();

        let last = *entities.last().unwrap();
        world.delete(last);

        for (i, e) in entities.iter().take(entities.len() - 1).enumerate() {
            match world.get_component(*e) {
                Some(x) => assert_eq!(components.get(i).map(|(x, _)| x), Some(&x as &Pos)),
                None => assert_eq!(components.get(i).map(|(x, _)| x), None),
            }
            match world.get_component(*e) {
                Some(x) => assert_eq!(components.get(i).map(|(_, x)| x), Some(&x as &Rot)),
                None => assert_eq!(components.get(i).map(|(_, x)| x), None),
            }
        }
    }

    #[test]
    fn delete_first() {
        let mut world = create();

        let shared = (Static, Model(5));
        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        let entities = world.insert(shared, components.clone()).to_vec();

        let first = *entities.first().unwrap();
        world.delete(first);

        for (i, e) in entities.iter().skip(1).enumerate() {
            match world.get_component(*e) {
                Some(x) => assert_eq!(components.get(i + 1).map(|(x, _)| x), Some(&x as &Pos)),
                None => assert_eq!(components.get(i + 1).map(|(x, _)| x), None),
            }
            match world.get_component(*e) {
                Some(x) => assert_eq!(components.get(i + 1).map(|(_, x)| x), Some(&x as &Rot)),
                None => assert_eq!(components.get(i + 1).map(|(_, x)| x), None),
            }
        }
    }

    #[test]
    fn add_component() {
        let mut world = create();

        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        let entities = world.insert((Static,), components.clone()).to_vec();

        for (i, e) in entities.iter().enumerate() {
            world.add_component(*e, Scale(2., 2., 2.));
            assert_eq!(
                components.get(i).unwrap().0,
                *world.get_component(*e).unwrap()
            );
            assert_eq!(
                components.get(i).unwrap().1,
                *world.get_component(*e).unwrap()
            );
            assert_eq!(Scale(2., 2., 2.), *world.get_component(*e).unwrap());
        }
    }

    #[test]
    fn remove_component() {
        let mut world = create();

        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        let entities = world.insert((Static,), components.clone()).to_vec();

        for (i, e) in entities.iter().enumerate() {
            world.remove_component::<Rot>(*e);
            assert_eq!(
                components.get(i).unwrap().0,
                *world.get_component(*e).unwrap()
            );
            assert!(world.get_component::<Rot>(*e).is_none());
        }
    }

    #[test]
    fn add_tag() {
        let mut world = create();

        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        let entities = world.insert((Static,), components.clone()).to_vec();

        for (i, e) in entities.iter().enumerate() {
            world.add_tag(*e, Model(2));
            assert_eq!(
                components.get(i).unwrap().0,
                *world.get_component(*e).unwrap()
            );
            assert_eq!(
                components.get(i).unwrap().1,
                *world.get_component(*e).unwrap()
            );
            assert_eq!(Static, *world.get_tag(*e).unwrap());
            assert_eq!(Model(2), *world.get_tag(*e).unwrap());
        }
    }

    #[test]
    fn remove_tag() {
        let mut world = create();

        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        let entities = world.insert((Static,), components.clone()).to_vec();

        for (i, e) in entities.iter().enumerate() {
            world.remove_tag::<Static>(*e);
            assert_eq!(
                components.get(i).unwrap().0,
                *world.get_component(*e).unwrap()
            );
            assert_eq!(
                components.get(i).unwrap().1,
                *world.get_component(*e).unwrap()
            );
            assert!(world.get_tag::<Static>(*e).is_none());
        }
    }
}
