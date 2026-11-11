//! The nodes whose waiting Boxes are being promoted right now.

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use moor_core::name::NodeName;

/// The nodes with a promotion running; one per controller, so two never run on one node.
#[derive(Default)]
pub(crate) struct InFlight {
    /// The nodes promoting now.
    nodes: Mutex<HashSet<NodeName>>,
    /// Signalled each time a promotion ends.
    ended: Condvar,
}

impl InFlight {
    /// Marks `node` as promoting until the returned claim drops; `None` when it already is.
    pub(crate) fn claim(self: &Arc<Self>, node: &NodeName) -> Option<Claim> {
        let mut nodes = self.nodes.lock().unwrap_or_else(PoisonError::into_inner);
        nodes.insert(node.clone()).then(|| Claim {
            set: Arc::clone(self),
            node: node.clone(),
        })
    }

    /// Waits until no node is promoting.
    #[cfg(test)]
    pub(crate) fn wait_idle(&self) {
        let nodes = self.nodes.lock().unwrap_or_else(PoisonError::into_inner);
        drop(
            self.ended
                .wait_while(nodes, |n| !n.is_empty())
                .unwrap_or_else(PoisonError::into_inner),
        );
    }
}

/// A node's promotion in flight; dropping it ends the promotion.
pub(crate) struct Claim {
    /// The set the node is in.
    set: Arc<InFlight>,
    /// The node promoting.
    node: NodeName,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.set
            .nodes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.node);
        self.set.ended.notify_all();
    }
}
