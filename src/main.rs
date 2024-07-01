use std::{collections::HashMap, marker::PhantomData, ops::BitAnd};

use crossbeam_channel::ReadyTimeoutError;
use legion::{
    internals::{
        iter::indexed::{TrustedRandomAccess, TrustedRandomAccessExt},
        query::{
            filter::component, view::IntoView,
            IBetYouDontKnowShitAboutThisFuckItTurnedOutActuallyIDontNeedThisBecauseICantJustUsePasstroghOnTheFilterButIStillWannaKeepThis,
        },
    },
    query::{
        And, ChunkView, ComponentFilter, DefaultFilter, DynamicFilter, EntityFilter,
        EntityFilterTuple, GroupMatcher, IntoIndexableIter, LayoutFilter, Not, Passthrough, View,
    },
    storage::Component,
    *,
};
use rayon::iter::IntoParallelIterator;

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
    world.extend(vector.clone());
    world.extend(vector2.clone());

    let mut query =
        <Read<Pos>>::query().filter(<EntityFilterTuple<Passthrough, Passthrough>>::default());
    let mut counter = 0;
    let mut entity_counter = 0;

    for chunk in query.iter_chunks(&world) {
        counter += 1;

        let chunk = chunk.get_indexable();

        for _entity in chunk {
            entity_counter += 1;
        }
    }
    dbg!(counter);
    dbg!(entity_counter);
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
