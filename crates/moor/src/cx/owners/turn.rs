//! The proof that an op runs under its Box's turn.

use std::marker::PhantomData;

use moor_core::name::BoxId;
use moor_core::ErrorKind;

use super::Owners;
use crate::error::CommandError;

/// One Box's turn, released on drop, also when its op panics.
///
/// It stays on the thread that was granted it; helper threads carry a [`TurnShare`].
pub(crate) struct Turn<'a> {
    /// Where the turn is released.
    owners: &'a Owners,
    /// The Box whose turn this is.
    box_id: BoxId,
    /// The ticket the turn was granted to.
    seq: u64,
    /// Keeps the turn on its thread, which re-entry detection relies on.
    _thread: PhantomData<*const ()>,
}

// A `Turn` that could cross threads would defeat re-entry detection, so this fails to compile
// once `Turn` is `Send`: both impls below would then apply and the call is ambiguous.
const _: () = {
    trait AmbiguousIfSend<A> {
        fn some_item() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    struct IsSend;
    impl<T: ?Sized + Send> AmbiguousIfSend<IsSend> for T {}
    let _ = <Turn<'static> as AmbiguousIfSend<_>>::some_item;
};

/// A helper thread's claim to act under another thread's turn; valid only while that turn lasts.
#[derive(Clone)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "no helper thread acts under a shared turn yet")
)]
pub(crate) struct TurnShare<'a> {
    /// Where the turn is looked up.
    owners: &'a Owners,
    /// The Box whose turn it is.
    box_id: BoxId,
    /// The ticket the turn was granted to; tickets are never reused.
    seq: u64,
}

/// A run whose exec goes on after its turn; the Box's owner counts it until this drops.
pub(crate) struct RunHold<'a> {
    /// Where the run is counted.
    owners: &'a Owners,
    /// The Box running it.
    box_id: BoxId,
}

impl<'a> Turn<'a> {
    /// Ticket `seq`'s turn on `box_id`, already recorded as its holder.
    pub(super) const fn new(owners: &'a Owners, box_id: BoxId, seq: u64) -> Self {
        Self {
            owners,
            box_id,
            seq,
            _thread: PhantomData,
        }
    }

    /// A claim on this turn for helper threads; it lapses when the turn ends.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no helper thread acts under a shared turn yet")
    )]
    pub(crate) const fn share(&self) -> TurnShare<'a> {
        TurnShare {
            owners: self.owners,
            box_id: self.box_id,
            seq: self.seq,
        }
    }

    /// Marks a run in flight on this Box until the returned hold drops, past the end of the turn.
    pub(crate) fn begin_run(&self) -> RunHold<'a> {
        if let Some(owner) = self.owners.lock().boxes.get_mut(&self.box_id) {
            owner.runs += 1;
        }
        RunHold {
            owners: self.owners,
            box_id: self.box_id,
        }
    }
}

impl TurnShare<'_> {
    /// Checks that the shared turn on this Box is still held.
    ///
    /// # Errors
    /// `Internal` once the turn has ended.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no helper thread acts under a shared turn yet")
    )]
    pub(crate) fn check(&self) -> Result<BoxId, CommandError> {
        let held = (self.owners.lock().boxes.get(&self.box_id))
            .and_then(|o| o.holder.as_ref())
            .is_some_and(|h| h.seq == self.seq);
        if held {
            return Ok(self.box_id);
        }
        Err(CommandError::new(
            ErrorKind::Internal,
            "acted on a Box after its turn ended",
        ))
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.owners.release(self.box_id, Some(self.seq));
    }
}

impl Drop for RunHold<'_> {
    fn drop(&mut self) {
        self.owners.release(self.box_id, None);
    }
}
