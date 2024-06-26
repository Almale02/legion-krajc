use std::{collections::HashMap, marker::PhantomData};

use crossbeam_channel::ReadyTimeoutError;
use legion::{
    internals::{
        iter::indexed::{TrustedRandomAccess, TrustedRandomAccessExt},
        query::{
            filter::component, view::IntoView,
            IBetYouDontKnowShitAboutThisFuckItTurnedOutActuallyIDontNeedThisBecauseICantJustUsePasstroghOnTheFilterButIStillWannaKeepThis,
        },
        storage::component,
    },
    query::{
        And, ChunkView, ComponentFilter, DefaultFilter, DynamicFilter, EntityFilter,
        EntityFilterTuple, GroupMatcher, IntoIndexableIter, LayoutFilter, Not, Passthrough, View,
    },
    storage::Component,
    *,
};
use rayon::iter::IntoParallelIterator;

trait GetShit {
    fn get_it<A: Iterator + DefaultFilter + IntoView, T: EntityFilter>()
    where
        A: std::ops::BitAnd<T>,
        <A as std::ops::BitAnd<T>>::Output: EntityFilter + IntoView + Iterator + DefaultFilter;
}

impl GetShit for i32 {
    fn get_it<A: IntoView + DefaultFilter, T: EntityFilter>() {
        let def2 = <Query<
            A,
            EntityFilterTuple<Not<ComponentFilter<IBetYouDontKnowShitAboutThisFuckItTurnedOutActuallyIDontNeedThisBecauseICantJustUsePasstroghOnTheFilterButIStillWannaKeepThis>>, Passthrough>,
        >>::new();

        let def3 = <Query<A, EntityFilterTuple<Passthrough, Passthrough>>>::new()
            .filter(!component::<Pos>());
    }
}

fn main() {
    let mut world = World::default();

    let mut vector: Vec<(Pos, Rot)> = vec![];
    let mut vector2: Vec<(Pos, Rot, Vel)> = vec![];

    for i in 0..9999999 {
        vector.push((Pos(i as f32, 0., 0.), Rot(0., i as f32, 0.)))
    }
    for i in 0..9999999 {
        vector2.push((
            Pos(i as f32, 0., 0.),
            Rot(0., i as f32, 0.),
            Vel(0., 0., i as f32),
        ))
    }
    let components = vector;
    let components2 = vector2;

    world.extend(components.clone());
    world.extend(components2.clone());
    let q = <Read<Pos>>::query().filter(!(component::<Pos>() & !component::<Rot>()));
    let mut query = <(Read<Pos>)>::query().filter(!(component::<Pos>() & !component::<Rot>()));

    let mut query = <(Read<Pos>, Entity)>::query().filter(component::<Entity>());
    unsafe {
        let mut counter = 0;
        let mut entity_counter = 0;
        for chunk in query.iter_chunks_unchecked(&world) {
            counter += 1;

            let mut chunk = chunk.get_indexable();

            let a = chunk.get(3);

            for entity in chunk {
                entity_counter += 1;
            }
        }
        dbg!(counter);
        dbg!(entity_counter);
    }
}
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
