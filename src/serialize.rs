//! Serde (de)serialization of worlds.
//!
//! As component types are not known at compile time, the world must be provided with the
//! means to serialize each component. This is provided by the
//! [WorldSerializer](trait.WorldSerializer.html) implementation. This implementation
//! also describes how [ComponentTypeIDs](../storage/struct.ComponentTypeId.html) (which
//! are not stable between compiles) are mapped to stable type identifiers. Components that are
//! not known to the serializer will be omitted from the serialized output.
//!
//! The [Registry](struct.Registry.html) provides a
//! [WorldSerializer](trait.WorldSerializer.html) implementation suitable for most
//! situations.
//!
//! Serializing all entities with a `Position` component to JSON.
//! ```
//! # use legion::*;
//! # use legion::serialize::Canon;
//! # let world = World::default();
//! # #[derive(serde::Serialize, serde::Deserialize)]
//! # struct Position;
//! // create a registry which uses strings as the external type ID
//! let mut registry = Registry::<String>::default();
//! registry.register::<Position>("position".to_string());
//! registry.register::<f32>("f32".to_string());
//! registry.register::<bool>("bool".to_string());
//!
//! // serialize entities with the `Position` component
//! let entity_serializer = Canon::default();
//! let json = serde_json::to_value(&world.as_serializable(component::<Position>(), &registry, &entity_serializer)).unwrap();
//! println!("{:#}", json);
//!
//! // registries are also serde deserializers
//! use serde::de::DeserializeSeed;
//! let world: World = registry.as_deserialize(&entity_serializer).deserialize(json).unwrap();
//! ```

pub use crate::internals::serialize::{
    de::WorldDeserializer,
    id::{Canon, CustomEntitySerializer, EntityName, EntitySerializer},
    ser::{SerializableWorld, WorldSerializer},
    AutoTypeKey, DeserializeIntoWorld, DeserializeNewWorld, Registry, TypeKey, UnknownType,
};

#[cfg(feature = "type-uuid")]
pub use crate::internals::serialize::SerializableTypeUuid;
