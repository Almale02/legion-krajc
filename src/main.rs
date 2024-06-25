use std::{collections::HashMap, marker::PhantomData};

use legion::{
    internals::iter::indexed::{TrustedRandomAccess, TrustedRandomAccessExt},
    query::{ChunkView, IntoIndexableIter},
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
    let components = vector;
    let components2 = vector2;

    world.extend(components.clone());
    world.extend(components2.clone());

    let mut query = <(Entity, Read<Pos>)>::query();
    unsafe {
        let mut counter = 0;
        let mut entity_counter = 0;
        for chunk in query.iter_chunks_unchecked(&world) {
            counter += 1;

            let mut chunk = chunk.get_indexable();

            chunk.get()

            for entity in chunk {
                entity_counter += 1;
            }
        }
        dbg!(counter);
        dbg!(entity_counter);
    }
    let mut count = 0;
    for (entity, pos) in query.iter_mut(&mut world) {
        count += 1;
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
