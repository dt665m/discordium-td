//! Independently typed, bounded actor resources. Games supply all capacities and rates.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

#[derive(Component, Debug, Serialize, Deserialize)]
#[serde(bound = "", try_from = "MeterValues", into = "MeterValues")]
pub struct Meter<K: Send + Sync + 'static> {
    current: f32,
    capacity: f32,
    marker: PhantomData<K>,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
struct MeterValues {
    current: f32,
    capacity: f32,
}
impl<K: Send + Sync + 'static> Clone for Meter<K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K: Send + Sync + 'static> Copy for Meter<K> {}
impl<K: Send + Sync + 'static> PartialEq for Meter<K> {
    fn eq(&self, other: &Self) -> bool {
        self.current == other.current && self.capacity == other.capacity
    }
}
impl<K: Send + Sync + 'static> From<Meter<K>> for MeterValues {
    fn from(meter: Meter<K>) -> Self {
        Self {
            current: meter.current,
            capacity: meter.capacity,
        }
    }
}
impl<K: Send + Sync + 'static> TryFrom<MeterValues> for Meter<K> {
    type Error = &'static str;
    fn try_from(v: MeterValues) -> Result<Self, Self::Error> {
        Self::new(v.current, v.capacity).ok_or("invalid meter values")
    }
}
impl<K: Send + Sync + 'static> Meter<K> {
    /// Reject non-finite, negative, or over-capacity initial state.
    pub fn new(current: f32, capacity: f32) -> Option<Self> {
        if !current.is_finite() || !capacity.is_finite() || current < 0.0 || capacity < current {
            return None;
        }
        Some(Self {
            current,
            capacity,
            marker: PhantomData,
        })
    }
    pub fn current(&self) -> f32 {
        self.current
    }
    pub fn capacity(&self) -> f32 {
        self.capacity
    }
    pub fn try_spend(&mut self, amount: f32) -> bool {
        if !amount.is_finite() || amount < 0.0 || amount > self.current {
            return false;
        }
        self.current -= amount;
        true
    }
    /// Returns the amount restored; invalid amounts leave state untouched.
    pub fn restore(&mut self, amount: f32) -> f32 {
        if !amount.is_finite() || amount < 0.0 {
            return 0.0;
        }
        let restored = amount.min(self.capacity - self.current);
        self.current = (self.current + restored).min(self.capacity);
        restored
    }
    /// Capacity reductions clamp current; increases do not grant resources.
    pub fn set_capacity(&mut self, capacity: f32) -> bool {
        if !capacity.is_finite() || capacity < 0.0 {
            return false;
        }
        self.capacity = capacity;
        self.current = self.current.min(capacity);
        true
    }
}
#[derive(Component, Debug, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Regeneration<K: Send + Sync + 'static> {
    pub per_second: f32,
    #[serde(skip)]
    marker: PhantomData<K>,
}
impl<K: Send + Sync + 'static> Regeneration<K> {
    pub fn new(per_second: f32) -> Self {
        Self {
            per_second,
            marker: PhantomData,
        }
    }
}
