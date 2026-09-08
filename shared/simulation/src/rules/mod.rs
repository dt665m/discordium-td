//! Pure shared gameplay math and state transitions.
mod attack;
mod charge;
mod navigation;
mod spatial;
mod targeting;
pub(crate) use attack::*;
pub(crate) use charge::*;
pub(crate) use navigation::*;
pub(crate) use spatial::*;
pub(crate) use targeting::*;
