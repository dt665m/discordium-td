//! Owned, decoded messages cross a bounded SPSC ring. Only scalar ring APIs are
//! exposed here: callers never wait for the opposite endpoint or commit chunks.
use game_shared::{ClientAck, ClientAction, ClientCommand, ClientMoveBundle, decode_with_limit};

pub(super) const RELIABLE_INPUT_LIMIT: usize = 16;
pub(super) const UNRELIABLE_INPUT_LIMIT: usize = 32;
// Room for each peer's fixed-tick command quota and connection lifecycle.
pub(super) const INPUTS_PER_CLIENT: usize = RELIABLE_INPUT_LIMIT + UNRELIABLE_INPUT_LIMIT + 2;
pub(crate) const MAX_INGRESS_PER_PASS: usize = 256;
pub(crate) const INGRESS_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(500);

pub(crate) type PeerMetrics = Vec<(u64, &'static str, Option<renet::NetworkInfo>)>;

pub(crate) enum Ingress {
    Connected(u64),
    Disconnected(u64),
    Action(u64, ClientAction),
    Moves(u64, ClientMoveBundle),
    Metrics(PeerMetrics),
}

pub(crate) enum DecodedInput {
    Command(Ingress),
    Ack(u32),
    Disconnect,
    Ignore,
}

/// Decode directly from Renet's borrowed Bytes contents, once. The resulting
/// vectors are moved into ECS; no Bytes -> Vec copy or second bundle decode.
/// Epoch, sequencing and gameplay acceptance still belong to simulation.
pub(crate) fn decode_input(id: u64, reliable: bool, bytes: &[u8]) -> DecodedInput {
    if reliable {
        if bytes.len() > 64 {
            return DecodedInput::Disconnect;
        }
        return match decode_with_limit::<ClientAction>(bytes, 64) {
            Ok(action) if matches!(action.command, ClientCommand::Move { .. }) => {
                DecodedInput::Disconnect
            }
            Ok(action) => DecodedInput::Command(Ingress::Action(id, action)),
            Err(_) => DecodedInput::Ignore,
        };
    }
    if bytes.len() > 2048 {
        return DecodedInput::Ignore;
    }
    match decode_with_limit::<ClientMoveBundle>(bytes, 2048) {
        Ok(bundle) if bundle.moves.len() <= 16 && bundle.actions.len() <= 64 => {
            DecodedInput::Command(Ingress::Moves(id, bundle))
        }
        Ok(_) => DecodedInput::Ignore,
        Err(_) => match decode_with_limit::<ClientAck>(bytes, 64) {
            Ok(ack) => DecodedInput::Ack(ack.tick),
            Err(_) => DecodedInput::Ignore,
        },
    }
}

pub(crate) struct IngressSender(rtrb::Producer<Ingress>);
pub(crate) struct IngressReceiver(rtrb::Consumer<Ingress>);

pub(crate) fn ingress_channel(capacity: usize) -> (IngressSender, IngressReceiver) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    (IngressSender(producer), IngressReceiver(consumer))
}

impl IngressSender {
    pub(crate) fn available(&self) -> usize {
        self.0.slots()
    }

    pub(crate) fn try_send(&mut self, input: Ingress) -> Result<(), Ingress> {
        self.0
            .push(input)
            .map_err(|rtrb::PushError::Full(input)| input)
    }
}

impl IngressReceiver {
    pub(crate) fn pending(&self) -> usize {
        self.0.slots()
    }

    pub(crate) fn try_recv(&mut self) -> Option<Ingress> {
        self.0.pop().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_shared::encode;

    fn bundle(seq: u32) -> ClientMoveBundle {
        ClientMoveBundle {
            match_epoch: 7,
            moves: vec![(seq, [1.0, 0.0])],
            actions: vec![],
        }
    }

    #[test]
    fn full_returns_ownership_and_wrapping_preserves_lifecycle_order() {
        let (mut sender, mut receiver) = ingress_channel(2);
        assert!(sender.try_send(Ingress::Connected(1)).is_ok());
        assert!(sender.try_send(Ingress::Disconnected(1)).is_ok());
        let moves = bundle(10);
        let allocation = moves.moves.as_ptr();
        let Err(event) = sender.try_send(Ingress::Moves(2, moves)) else {
            panic!()
        };
        assert!(matches!(receiver.try_recv(), Some(Ingress::Connected(1))));
        assert!(sender.try_send(event).is_ok());
        drop(sender);
        assert!(matches!(
            receiver.try_recv(),
            Some(Ingress::Disconnected(1))
        ));
        let Some(Ingress::Moves(2, moves)) = receiver.try_recv() else {
            panic!()
        };
        assert_eq!(
            moves.moves.as_ptr(),
            allocation,
            "handoff moves the allocation without copying"
        );
        assert_eq!(moves, bundle(10));
        assert!(receiver.try_recv().is_none());
    }

    #[test]
    fn threaded_handoff_reuses_bounded_storage_without_busy_waiting() {
        let (mut sender, mut receiver) = ingress_channel(4);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            scope.spawn(move || {
                for batch in 0..100 {
                    for seq in batch * 4..batch * 4 + 4 {
                        assert!(sender.try_send(Ingress::Moves(1, bundle(seq))).is_ok());
                    }
                    ready_tx.send(()).unwrap();
                    done_rx.recv().unwrap();
                }
            });
            for batch in 0..100 {
                ready_rx.recv().unwrap();
                assert_eq!(receiver.pending(), 4);
                for seq in batch * 4..batch * 4 + 4 {
                    let Some(Ingress::Moves(1, moves)) = receiver.try_recv() else {
                        panic!()
                    };
                    assert_eq!(moves, bundle(seq));
                }
                assert!(receiver.try_recv().is_none());
                done_tx.send(()).unwrap();
            }
        });
    }

    #[test]
    fn decode_preserves_payloads_and_bounds_bundle_expansion() {
        let expected = bundle(1);
        let DecodedInput::Command(Ingress::Moves(42, actual)) =
            decode_input(42, false, &encode(&expected))
        else {
            panic!()
        };
        assert_eq!(actual, expected);
        let mut oversized = expected;
        oversized.moves = (0..17).map(|seq| (seq, [0.0; 2])).collect();
        assert!(matches!(
            decode_input(1, false, &encode(&oversized)),
            DecodedInput::Ignore
        ));
        assert!(matches!(
            decode_input(1, false, &encode(&ClientAck { tick: 12 })),
            DecodedInput::Ack(12)
        ));
        assert!(matches!(
            decode_input(1, true, &[0; 65]),
            DecodedInput::Disconnect
        ));
        assert!(matches!(
            decode_input(1, false, &[0; 2049]),
            DecodedInput::Ignore
        ));
        assert!(matches!(
            decode_input(1, false, &[255]),
            DecodedInput::Ignore
        ));
        assert!(matches!(
            decode_input(1, true, &[255]),
            DecodedInput::Ignore
        ));
    }
}
