use super::*;
use crate::{codec::BoundedVec, types::EntityId};
use std::{collections::BTreeSet, marker::PhantomData};
pub type Capture<S, W> = fn(&W) -> Result<S, SchemaError>;
pub type RestoreReplacement<S, W> = fn(&W, &S) -> Result<W, SchemaError>;
pub enum DependencyDeclaration<S> {
    Unspecified,
    NoneRequired,
    /// Dependencies are resolved from authenticated external group context.
    External {
        policy: u32,
    },
    Declared {
        policy: u32,
        collect: fn(&S) -> Result<BoundedVec<EntityId, 64>, SchemaError>,
    },
}
#[derive(Clone, Copy, Debug)]
pub enum LifecycleDeclaration {
    StaticOnly,
    External { policy: u32 },
}
pub struct Registration<S, W> {
    pub capture: Capture<S, W>,
    /// Produces a replacement; the library never lends this callback `&mut W`.
    pub restore: Option<RestoreReplacement<S, W>>,
    pub dependencies: DependencyDeclaration<S>,
    pub lifecycle: LifecycleDeclaration,
}
pub struct RegisteredSchema<S: Schema, W> {
    codec: SchemaCodec<S>,
    hooks: Registration<S, W>,
}
impl<S: Schema, W> RegisteredSchema<S, W> {
    pub fn codec(&self) -> &SchemaCodec<S> {
        &self.codec
    }
    pub fn capture(&self, world: &W) -> Result<S, SchemaError> {
        let value = (self.hooks.capture)(world)?;
        self.codec.encode(&value)?;
        Ok(value)
    }
    /// Returns staged replacement state. Recapturing detects missing registered
    /// fields, but hidden gameplay state and callback purity still need game tests.
    pub fn restore_replacement(&self, world: &W, state: &S) -> Result<W, SchemaError> {
        let expected = self.codec.encode(state)?;
        let replacement = self.hooks.restore.ok_or(SchemaError::NoRestore)?(world, state)?;
        let restored = (self.hooks.capture)(&replacement)?;
        if self.codec.encode(&restored)? != expected {
            return Err(SchemaError::RestoreMismatch);
        }
        Ok(replacement)
    }
    /// This is a game declaration, not proof of runtime dependency completeness
    /// or a graph/disclosure grant. The prediction layer must close it separately.
    pub fn declared_dependencies(
        &self,
        state: &S,
    ) -> Result<BoundedVec<EntityId, 64>, SchemaError> {
        self.codec.encode(state)?;
        let mut dependencies = match self.hooks.dependencies {
            DependencyDeclaration::NoneRequired => Vec::new(),
            DependencyDeclaration::External { .. } => {
                return Err(SchemaError::DependencyContextRequired);
            }
            DependencyDeclaration::Declared { collect, .. } => collect(state)?.into_vec(),
            DependencyDeclaration::Unspecified => {
                return Err(SchemaError::MissingDependencyDeclaration);
            }
        };
        dependencies.sort_unstable();
        if dependencies
            .iter()
            .any(|id| id.index == 0 || id.generation == 0)
            || dependencies.windows(2).any(|p| p[0] == p[1])
        {
            return Err(SchemaError::InvalidDependency);
        }
        BoundedVec::new(dependencies).map_err(|_| SchemaError::CountBudget)
    }
    /// An external policy must be resolved by the authenticated game group.
    pub fn external_dependency_policy(&self) -> Option<u32> {
        match self.hooks.dependencies {
            DependencyDeclaration::External { policy } => Some(policy),
            _ => None,
        }
    }
    pub fn lifecycle_declaration(&self) -> LifecycleDeclaration {
        self.hooks.lifecycle
    }
}
pub struct SchemaRegistry {
    limits: SchemaLimits,
    entries: Vec<SchemaIdentity>,
    roots: BTreeSet<SchemaId>,
}
impl SchemaRegistry {
    pub fn new(limits: SchemaLimits) -> Result<Self, SchemaError> {
        Ok(Self {
            limits: limits.validate()?,
            entries: Vec::new(),
            roots: BTreeSet::new(),
        })
    }
    pub fn identities(&self) -> &[SchemaIdentity] {
        &self.entries
    }
    pub fn register<S: Schema, W>(
        &mut self,
        hooks: Registration<S, W>,
    ) -> Result<RegisteredSchema<S, W>, SchemaError> {
        if self.roots.contains(&S::ID) {
            return Err(SchemaError::DuplicateSchema);
        }
        let mut inspection = Inspection::new(self.limits);
        inspection.schema::<S>(0, Visibility::ServerOnly, Prediction::Replayed)?;
        if inspection.restore_required && hooks.restore.is_none() {
            return Err(SchemaError::MissingRestore);
        }
        match hooks.dependencies {
            DependencyDeclaration::Unspecified => {
                return Err(SchemaError::MissingDependencyDeclaration);
            }
            DependencyDeclaration::Declared { policy: 0, .. }
            | DependencyDeclaration::External { policy: 0 } => {
                return Err(SchemaError::MissingDependencyDeclaration);
            }
            _ => {}
        }
        if matches!(
            hooks.lifecycle,
            LifecycleDeclaration::External { policy: 0 }
        ) {
            return Err(SchemaError::InvalidLifecycleDeclaration);
        }
        let identity = SchemaIdentity {
            schema: S::ID,
            version: S::VERSION,
            fingerprint: inspection.fingerprint(),
        };
        let mut additions = Vec::new();
        for candidate in &inspection.known {
            match self
                .entries
                .binary_search_by_key(&candidate.schema, |e| e.schema)
            {
                Ok(i) if self.entries[i] != *candidate => return Err(SchemaError::DuplicateSchema),
                Ok(_) => {}
                Err(_) => additions.push(*candidate),
            }
        }
        if self.entries.len() + additions.len() > self.limits.schemas {
            return Err(SchemaError::CountBudget);
        }
        self.entries.extend(additions);
        self.entries.sort_by_key(|e| e.schema);
        self.roots.insert(S::ID);
        Ok(RegisteredSchema {
            codec: SchemaCodec {
                identity,
                limits: self.limits,
                marker: PhantomData,
            },
            hooks,
        })
    }
    /// Strict exact set/fingerprint negotiation; unknown optional fields are not
    /// silently skipped by this initial codec implementation.
    pub fn require_exact_match(&self, peer: &[SchemaIdentity]) -> Result<(), SchemaError> {
        if peer.len() > self.limits.schemas {
            return Err(SchemaError::CountBudget);
        }
        if peer.len() != self.entries.len() {
            return Err(SchemaError::IncompatibleSchema);
        }
        let mut sorted = peer.to_vec();
        sorted.sort_by_key(|e| e.schema);
        if sorted != self.entries {
            return Err(SchemaError::IncompatibleSchema);
        }
        Ok(())
    }
}
