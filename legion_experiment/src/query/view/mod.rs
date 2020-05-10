use super::{
    filter::{and::And, EntityFilter, EntityFilterTuple},
    QueryResult,
};
use crate::{
    iter::{
        indexed::{IndexedIter, TrustedRandomAccess},
        map::MapInto,
        zip::{multizip, Zip},
    },
    storage::{
        archetype::Archetype,
        component::{Component, ComponentTypeId},
        Components,
    },
};
use std::marker::PhantomData;

pub mod entity;
pub mod read;
pub mod try_read;
pub mod try_write;
pub mod write;

// View and Fetch types are separate traits so that View implementations can be
// zero sized types and therefore not need the user to provide a lifetime when they
// declare queries.

pub trait DefaultFilter {
    type Filter: EntityFilter;
}

/// A type which can pull entitiy data out of a world.
pub trait View<'data>: DefaultFilter + Sized {
    type Element: Send + Sync;
    /// The fetch type yielded for each archetype.
    type Fetch: Fetch + IntoIndexableIter<Item = Self::Element> + 'data;
    /// The iterator type which pulls entity data out of a world.
    type Iter: Iterator<Item = Self::Fetch> + 'data;
    /// Contains the type IDs read by the view.
    type Read: AsRef<[ComponentTypeId]>;
    /// Contains the type IDs written by the view.
    type Write: AsRef<[ComponentTypeId]>;

    /// Creates an iterator which will yield slices of entity data for each archetype.
    ///
    /// # Safety
    ///
    /// This method may return mutable references to entity data via shared world references.
    /// The caller must ensure that no two view iterators are alive at the same time which access
    /// any components in a manor which may cause mutable aliasing.
    unsafe fn fetch(
        components: &'data Components,
        archetypes: &'data [Archetype],
        query: QueryResult<'data>,
    ) -> Self::Iter;

    /// Determines if this view type is valid. Panics if checks fail.
    fn validate();

    fn reads_types() -> Self::Read;

    fn writes_types() -> Self::Write;

    /// Determines if the view reads the specified data type.
    fn reads<T: Component>() -> bool;

    /// Determines if the view writes to the specified data type.
    fn writes<T: Component>() -> bool;
}

pub trait IntoIndexableIter {
    type Item: Send + Sync;
    type IntoIter: Iterator<Item = Self::Item>
        + TrustedRandomAccess<Item = Self::Item>
        + Send
        + Sync;

    fn into_indexable_iter(self) -> Self::IntoIter;
}

/// A type which holds onto a slice of entitiy data retrieved from a single archetype.
pub trait Fetch: IntoIndexableIter + Send + Sync {
    type Data;

    /// Converts the fetch into the retrieved component slices
    fn into_components(self) -> Self::Data;

    /// Tries to find a slice of components, if this fetch contains the
    /// requested component type.
    fn find<T: 'static>(&self) -> Option<&[T]>;

    /// Tries to find a mutable slice of components, if this fetch contains
    /// the requested component type.
    fn find_mut<T: 'static>(&mut self) -> Option<&mut [T]>;

    /// Tries to find the component slice version of a component,
    /// if this fetch contains the requested component type.
    fn version<T: Component>(&self) -> Option<u64>;

    /// Indicates that the archetype is going to be provided to the user.
    /// Component slice versions are incremented here.
    fn accepted(&mut self);
}

// A fetch which only retrieves shared references to component data.
pub unsafe trait ReadOnlyFetch: Fetch {
    fn get_components(&self) -> Self::Data;
}

/// A marker trait which marks types which only perform data reads.
#[doc(hidden)]
pub unsafe trait ReadOnly {}

unsafe impl<T> ReadOnly for &T {}
unsafe impl<T> ReadOnly for Option<&T> {}

pub struct MultiFetch<'a, T> {
    fetches: T,
    _phantom: PhantomData<&'a T>,
}

impl<'a, T> From<T> for MultiFetch<'a, T> {
    fn from(value: T) -> Self {
        Self {
            fetches: value,
            _phantom: PhantomData,
        }
    }
}

macro_rules! view_tuple {
    ($head_ty:ident) => {
        impl_view_tuple!($head_ty);
    };
    ($head_ty:ident, $( $tail_ty:ident ),*) => (
        impl_view_tuple!($head_ty, $( $tail_ty ),*);
        view_tuple!($( $tail_ty ),*);
    );
}

macro_rules! impl_view_tuple {
    ( $( $ty: ident ),* ) => {
        impl<$( $ty: DefaultFilter ),*> DefaultFilter for ($( $ty, )*) {
            type Filter = EntityFilterTuple<
                And<($( <$ty::Filter as EntityFilter>::Layout, )*)>,
                And<($( <$ty::Filter as EntityFilter>::Dynamic, )*)>
            >;
        }

        impl<'a, $( $ty: View<'a> + 'a ),*> View<'a> for ($( $ty, )*) {
            type Element = <Self::Fetch as IntoIndexableIter>::Item;
            type Fetch = <Self::Iter as Iterator>::Item;
            type Iter = MapInto<Zip<($( $ty::Iter, )*)>, MultiFetch<'a, ($( $ty::Fetch, )*)>>;
            type Read = Vec<ComponentTypeId>;
            type Write = Vec<ComponentTypeId>;

            unsafe fn fetch(
                components: &'a Components,
                archetypes: &'a [Archetype],
                query: QueryResult<'a>,
            ) -> Self::Iter {
                MapInto::new(
                    multizip(
                        (
                            $( $ty::fetch(components, archetypes, query.clone()), )*
                        )
                    )
                )
            }

            paste::item! {
                fn validate() {
                    #![allow(non_snake_case)]
                    $( let [<$ty _reads>] = $ty::reads_types(); )*
                    $( let [<$ty _writes>] = $ty::writes_types(); )*
                    let reads = [$( [<$ty _reads>].as_ref(), )*];
                    let writes = [$( [<$ty _writes>].as_ref(), )*];

                    for (i, writes) in writes.iter().enumerate() {
                        for (j, other_reads) in reads.iter().enumerate() {
                            if i == j { continue; }
                            for w in writes.iter() {
                                assert!(!other_reads.iter().any(|x| x == w));
                            }
                        }
                    }
                }

                fn reads_types() -> Self::Read {
                    #![allow(non_snake_case)]
                    let types = std::iter::empty();
                    $( let [<$ty _reads>] = $ty::reads_types(); )*
                    $( let types = types.chain([<$ty _reads>].as_ref().iter()); )*
                    types.copied().collect()
                }

                fn writes_types() -> Self::Write {
                    #![allow(non_snake_case)]
                    let types = std::iter::empty();
                    $( let [<$ty _reads>] = $ty::reads_types(); )*
                    $( let types = types.chain([<$ty _reads>].as_ref().iter()); )*
                    types.copied().collect()
                }
            }

            fn reads<Comp: Component>() -> bool {
                $(
                    $ty::reads::<Comp>()
                )||*
            }

            fn writes<Comp: Component>() -> bool {
                $(
                    $ty::writes::<Comp>()
                )||*
            }
        }

        impl<'a, $( $ty: Fetch ),*> IntoIndexableIter for MultiFetch<'a, ($( $ty, )*)> {
            type IntoIter = IndexedIter<($( $ty::IntoIter, )*)>;
            type Item = <Self::IntoIter as Iterator>::Item;

            fn into_indexable_iter(self) -> Self::IntoIter {
                #[allow(non_snake_case)]
                let ($( $ty, )*) = self.fetches;
                IndexedIter::new(($( $ty.into_indexable_iter(), )*))
            }
        }

        impl<'a, $( $ty: Fetch ),*> IntoIterator for MultiFetch<'a, ($( $ty, )*)> {
            type IntoIter = <Self as IntoIndexableIter>::IntoIter;
            type Item = <Self as IntoIndexableIter>::Item;

            fn into_iter(self) -> Self::IntoIter {
                self.into_indexable_iter()
            }
        }

        unsafe impl<'a, $( $ty: ReadOnlyFetch),*> ReadOnlyFetch for MultiFetch<'a, ($( $ty, )*)>
        {
            fn get_components(&self) -> Self::Data {
                #[allow(non_snake_case)]
                let ($( $ty, )*) = &self.fetches;
                (($( $ty.get_components(), )*))
            }
        }

        impl<'a, $( $ty: Fetch ),*> Fetch for MultiFetch<'a, ($( $ty, )*)> {
            type Data = ($( $ty::Data, )*);

            #[inline]
            fn into_components(self) -> Self::Data {
                #[allow(non_snake_case)]
                let ($( $ty, )*) = self.fetches;
                ($( $ty.into_components(), )*)
            }

            #[inline]
            fn find<Comp: 'static>(&self) -> Option<&[Comp]> {
                #[allow(non_snake_case)]
                let ($( $ty, )*) = &self.fetches;
                let mut result = None;
                $(
                    result = result.or_else(|| $ty.find());
                )*
                result
            }

            #[inline]
            fn find_mut<Comp: 'static>(&mut self) -> Option<&mut [Comp]> {
                #[allow(non_snake_case)]
                let ($( $ty, )*) = &mut self.fetches;
                let mut result = None;
                $(
                    result = result.or_else(move || $ty.find_mut());
                )*
                result
            }

            #[inline]
            fn version<Comp: Component>(&self) -> Option<u64> {
                #[allow(non_snake_case)]
                let ($( $ty, )*) = &self.fetches;
                let mut result = None;
                $(
                    result = result.or_else(|| $ty.version::<Comp>());
                )*
                result
            }

            #[inline]
            fn accepted(&mut self) {
                #[allow(non_snake_case)]
                let ($( $ty, )*) = &mut self.fetches;
                $( $ty.accepted(); )*
            }
        }
    };
}

view_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z);
