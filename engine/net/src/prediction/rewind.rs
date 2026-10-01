//! Rollback prediction for one locally controlled actor.
//!
//! The client steps its actor ahead of the server with the same simulation
//! code, one fixed tick at a time, and records each tick's input and the
//! dependencies it read. When the server's state for tick K arrives, the
//! history from K onward is replayed from that state with the recorded inputs.
//! A state older than the retained history, or newer than the prediction, is
//! simply adopted as the new base: nothing in that case is a fault.
use crate::types::ServerTick;
use std::collections::VecDeque;

/// The input a tick was stepped with. A substitute repeats whatever the
/// server would apply when no command arrived for that tick.
#[derive(Debug, Clone, PartialEq)]
pub enum RewindInput<C> {
    Command(C),
    Substitute,
}
impl<C> RewindInput<C> {
    pub fn command(&self) -> Option<&C> {
        match self {
            Self::Command(command) => Some(command),
            Self::Substitute => None,
        }
    }
}

/// A game's prediction step. `Dependencies` is whatever else a tick reads,
/// such as collision geometry or moving platforms; it is recorded with the
/// tick so a replay sees exactly what the original step saw.
pub trait RewindModel {
    type State: Clone;
    type Command: Clone;
    type Dependencies: Clone;
    type Error;
    fn step(
        &self,
        state: &mut Self::State,
        tick: ServerTick,
        input: &RewindInput<Self::Command>,
        dependencies: &Self::Dependencies,
    ) -> Result<(), Self::Error>;
}

struct Frame<M: RewindModel> {
    tick: ServerTick,
    state: M::State,
    /// `None` for the authoritative base.
    input: Option<RewindInput<M::Command>>,
    dependencies: M::Dependencies,
}

/// How an authoritative state was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Correction {
    /// Restored at its tick and replayed this many ticks of local input.
    Replayed(usize),
    /// Outside the retained history: became the new base.
    Rebased,
    /// Older than the current base; ignored.
    Stale,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RewindWork {
    pub predicted_ticks: usize,
    pub replayed_ticks: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewindError<E> {
    /// The tick does not follow the prediction.
    Order,
    Uninitialized,
    Model(E),
}

pub struct Rewind<M: RewindModel> {
    model: M,
    frames: VecDeque<Frame<M>>,
    history_ticks: usize,
    work: RewindWork,
}
impl<M: RewindModel> Rewind<M> {
    /// `history_ticks` bounds how far prediction may run ahead of the newest
    /// authoritative state and still be replayed.
    pub fn new(model: M, history_ticks: usize) -> Self {
        Self {
            model,
            frames: VecDeque::new(),
            history_ticks: history_ticks.max(1),
            work: RewindWork::default(),
        }
    }
    pub fn model(&self) -> &M {
        &self.model
    }
    pub fn model_mut(&mut self) -> &mut M {
        &mut self.model
    }
    pub fn is_initialized(&self) -> bool {
        !self.frames.is_empty()
    }
    /// Discard everything and start from an authoritative state.
    pub fn reset(&mut self, tick: ServerTick, state: M::State, dependencies: M::Dependencies) {
        self.frames.clear();
        self.frames.push_back(Frame {
            tick,
            state,
            input: None,
            dependencies,
        });
    }
    pub fn clear(&mut self) {
        self.frames.clear();
    }
    /// Per-frame accounting for diagnostics; call once per rendered frame.
    pub fn begin_frame(&mut self) {
        self.work = RewindWork::default();
    }
    pub fn work(&self) -> RewindWork {
        self.work
    }
    pub fn predicted_tick(&self) -> Option<ServerTick> {
        self.frames.back().map(|frame| frame.tick)
    }
    /// Tick of the authoritative state the history starts from.
    pub fn base_tick(&self) -> Option<ServerTick> {
        self.frames.front().map(|frame| frame.tick)
    }
    pub fn state(&self) -> Option<&M::State> {
        self.frames.back().map(|frame| &frame.state)
    }
    pub fn base_state(&self) -> Option<&M::State> {
        self.frames.front().map(|frame| &frame.state)
    }
    pub fn base_dependencies(&self) -> Option<&M::Dependencies> {
        self.frames.front().map(|frame| &frame.dependencies)
    }
    fn frame(&self, tick: ServerTick) -> Option<&Frame<M>> {
        let first = self.frames.front()?.tick;
        let index = usize::try_from(tick.0.checked_sub(first.0)?).ok()?;
        self.frames.get(index)
    }
    pub fn history_state(&self, tick: ServerTick) -> Option<&M::State> {
        self.frame(tick).map(|frame| &frame.state)
    }
    pub fn history_input(&self, tick: ServerTick) -> Option<&RewindInput<M::Command>> {
        self.frame(tick)?.input.as_ref()
    }
    pub fn history_dependencies(&self, tick: ServerTick) -> Option<&M::Dependencies> {
        self.frame(tick).map(|frame| &frame.dependencies)
    }
    /// Ticks prediction may still advance before the oldest retained input
    /// would have to be discarded.
    pub fn remaining_ticks(&self) -> usize {
        (self.history_ticks + 1).saturating_sub(self.frames.len())
    }
    /// Step the prediction to `tick`, which must follow the predicted tick.
    pub fn predict(
        &mut self,
        tick: ServerTick,
        input: RewindInput<M::Command>,
        dependencies: M::Dependencies,
    ) -> Result<(), RewindError<M::Error>> {
        let last = self.frames.back().ok_or(RewindError::Uninitialized)?;
        if last.tick.checked_next() != Some(tick) {
            return Err(RewindError::Order);
        }
        let mut state = last.state.clone();
        self.model
            .step(&mut state, tick, &input, &dependencies)
            .map_err(RewindError::Model)?;
        self.frames.push_back(Frame {
            tick,
            state,
            input: Some(input),
            dependencies,
        });
        // Prediction past the retained window keeps going; the oldest local
        // input is dropped, and a correction older than what remains rebases.
        while self.frames.len() > self.history_ticks + 1 {
            self.frames.pop_front();
        }
        self.work.predicted_ticks += 1;
        Ok(())
    }
    /// Apply the server's state for `tick` and replay local input after it.
    pub fn correct(
        &mut self,
        tick: ServerTick,
        state: M::State,
        dependencies: M::Dependencies,
    ) -> Result<Correction, RewindError<M::Error>> {
        let Some(first) = self.frames.front().map(|frame| frame.tick) else {
            self.reset(tick, state, dependencies);
            return Ok(Correction::Rebased);
        };
        let last = self.frames.back().map(|frame| frame.tick).unwrap_or(first);
        if tick < first {
            // The base is already newer, unless prediction has run so far ahead
            // that the history no longer reaches back to it.
            return Ok(Correction::Stale);
        }
        if tick > last {
            self.reset(tick, state, dependencies);
            return Ok(Correction::Rebased);
        }
        let index = usize::try_from(tick.0 - first.0).unwrap_or(usize::MAX);
        self.frames.drain(..index);
        let base = self
            .frames
            .front_mut()
            .expect("frame at the corrected tick");
        base.state = state;
        base.input = None;
        base.dependencies = dependencies;
        let mut replayed = 0;
        let mut previous = self.frames[0].state.clone();
        for index in 1..self.frames.len() {
            let frame = &mut self.frames[index];
            let input = frame.input.as_ref().expect("replayed frames carry input");
            if let Err(error) =
                self.model
                    .step(&mut previous, frame.tick, input, &frame.dependencies)
            {
                // Keep the corrected base; later local frames are unusable.
                self.frames.truncate(index);
                self.work.replayed_ticks += replayed;
                return Err(RewindError::Model(error));
            }
            frame.state = previous.clone();
            replayed += 1;
        }
        self.work.replayed_ticks += replayed;
        Ok(Correction::Replayed(replayed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Position integrates velocity; a command sets velocity, a substitute keeps
    /// it. The dependency is a wall the position cannot pass.
    struct Mover;
    impl RewindModel for Mover {
        type State = (i64, i64);
        type Command = i64;
        type Dependencies = i64;
        type Error = &'static str;
        fn step(
            &self,
            state: &mut (i64, i64),
            _: ServerTick,
            input: &RewindInput<i64>,
            wall: &i64,
        ) -> Result<(), &'static str> {
            if let RewindInput::Command(velocity) = input {
                if velocity.abs() > 100 {
                    return Err("too fast");
                }
                state.1 = *velocity;
            }
            state.0 = (state.0 + state.1).min(*wall);
            Ok(())
        }
    }

    fn predicted(ticks: u64) -> Rewind<Mover> {
        let mut rewind = Rewind::new(Mover, 8);
        rewind.reset(ServerTick(10), (0, 0), 1000);
        for tick in 11..=10 + ticks {
            rewind
                .predict(ServerTick(tick), RewindInput::Command(2), 1000)
                .unwrap();
        }
        rewind
    }

    #[test]
    fn corrections_replay_local_input_from_the_corrected_tick() {
        let mut rewind = predicted(5);
        assert_eq!(rewind.state(), Some(&(10, 2)));
        // The server saw a shove at tick 12: replay 13..=15 from there.
        assert_eq!(
            rewind.correct(ServerTick(12), (50, 2), 1000),
            Ok(Correction::Replayed(3))
        );
        assert_eq!(rewind.state(), Some(&(56, 2)));
        assert_eq!(rewind.base_tick(), Some(ServerTick(12)));
        assert_eq!(
            rewind.history_input(ServerTick(13)),
            Some(&RewindInput::Command(2))
        );
        // Replay reads each tick's recorded dependency, not today's.
        let mut walled = Rewind::new(Mover, 8);
        walled.reset(ServerTick(0), (0, 0), 1000);
        walled
            .predict(ServerTick(1), RewindInput::Command(5), 3)
            .unwrap();
        walled
            .predict(ServerTick(2), RewindInput::Substitute, 1000)
            .unwrap();
        assert_eq!(
            walled.correct(ServerTick(0), (1, 0), 1000),
            Ok(Correction::Replayed(2))
        );
        assert_eq!(walled.state(), Some(&(8, 5)));
    }

    #[test]
    fn states_outside_the_history_rebase_instead_of_failing() {
        let mut rewind = predicted(3);
        // Ahead of everything predicted, as after a long hitch.
        assert_eq!(
            rewind.correct(ServerTick(40), (7, 0), 1000),
            Ok(Correction::Rebased)
        );
        assert_eq!(rewind.predicted_tick(), Some(ServerTick(40)));
        assert_eq!(rewind.state(), Some(&(7, 0)));
        // Older than the base: a late packet.
        assert_eq!(
            rewind.correct(ServerTick(39), (0, 0), 1000),
            Ok(Correction::Stale)
        );
        // Prediction running past the window drops the oldest input; a
        // correction older than what remains is stale, a newer one replays.
        let mut far = predicted(20);
        assert_eq!(far.base_tick(), Some(ServerTick(22)));
        assert_eq!(far.remaining_ticks(), 0);
        assert_eq!(
            far.correct(ServerTick(15), (0, 0), 1000),
            Ok(Correction::Stale)
        );
        assert_eq!(
            far.correct(ServerTick(25), (0, 2), 1000),
            Ok(Correction::Replayed(5))
        );
    }

    #[test]
    fn prediction_is_strictly_tick_by_tick() {
        let mut rewind = predicted(1);
        assert_eq!(
            rewind.predict(ServerTick(13), RewindInput::Substitute, 0),
            Err(RewindError::Order)
        );
        let mut empty = Rewind::new(Mover, 8);
        assert_eq!(
            empty.predict(ServerTick(1), RewindInput::Substitute, 0),
            Err(RewindError::Uninitialized)
        );
        assert_eq!(
            empty.correct(ServerTick(5), (1, 1), 0),
            Ok(Correction::Rebased)
        );
    }

    #[test]
    fn a_failing_replay_keeps_the_corrected_base() {
        let mut rewind = predicted(2);
        rewind
            .predict(ServerTick(13), RewindInput::Command(500), 1000)
            .unwrap_err();
        rewind
            .predict(ServerTick(13), RewindInput::Command(3), 1000)
            .unwrap();
        assert_eq!(
            rewind.correct(ServerTick(11), (0, 0), 1000),
            Ok(Correction::Replayed(2))
        );
        assert_eq!(rewind.predicted_tick(), Some(ServerTick(13)));
    }
}
