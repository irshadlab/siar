//! §28 hierarchical accounting, §144 enterprise tenant quotas, §145
//! tenant isolation.
//!
//! [`AccountTree`]: every charge lands on a leaf and on *all* its
//! ancestors, atomically (all-or-nothing). When a limit refuses, the error
//! carries the full path of the node that said no ("files/bob/transfer-123"
//! blocked by "files/bob") — quota enforcement you can explain.
//!
//! [`TenantPools`]: a tenant's reserved critical budget is exclusive to
//! that tenant's critical work. Everyone's normal work competes only for
//! the shared remainder, so tenant A cannot touch tenant B's reserve.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------- tree

/// Generation-tagged so a stale id from a removed node can never alias a
/// new node that reused the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

#[derive(Debug)]
struct Node {
    name: String,
    parent: Option<NodeId>,
    limit: u64,
    used: u64,
    children: u32,
}

#[derive(Debug, Default)]
struct Slot {
    generation: u32,
    node: Option<Node>,
}

#[derive(Debug)]
struct Tree {
    slots: Vec<Slot>,
    free: Vec<u32>,
    live: usize,
    max_nodes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
    #[error("unknown or removed account node")]
    UnknownNode,
    #[error("account tree is full ({0} nodes)")]
    TreeFull(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{refused_by} refused: limit {limit}, in use {used}, requested {requested}")]
pub struct ChargeDenied {
    /// Slash-joined path of the node whose limit was hit.
    pub refused_by: String,
    pub limit: u64,
    pub used: u64,
    pub requested: u64,
}

#[derive(Debug, Clone)]
pub struct AccountTree(Arc<Mutex<Tree>>);

#[derive(Debug)]
#[must_use = "dropping the charge refunds it to every ancestor"]
pub struct Charge {
    tree: Arc<Mutex<Tree>>,
    leaf: NodeId,
    amount: u64,
}

impl Tree {
    fn get(&self, id: NodeId) -> Result<&Node, AccountError> {
        self.slots
            .get(id.index as usize)
            .filter(|s| s.generation == id.generation)
            .and_then(|s| s.node.as_ref())
            .ok_or(AccountError::UnknownNode)
    }

    fn get_mut(&mut self, id: NodeId) -> Result<&mut Node, AccountError> {
        self.slots
            .get_mut(id.index as usize)
            .filter(|s| s.generation == id.generation)
            .and_then(|s| s.node.as_mut())
            .ok_or(AccountError::UnknownNode)
    }

    fn insert(&mut self, node: Node) -> Result<NodeId, AccountError> {
        if self.live >= self.max_nodes {
            return Err(AccountError::TreeFull(self.max_nodes));
        }
        self.live += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.node = Some(node);
            Ok(NodeId {
                index,
                generation: slot.generation,
            })
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 0,
                node: Some(node),
            });
            Ok(NodeId {
                index,
                generation: 0,
            })
        }
    }

    fn path(&self, id: NodeId) -> String {
        let mut names = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            match self.get(c) {
                Ok(n) => {
                    names.push(n.name.as_str());
                    cur = n.parent;
                }
                Err(_) => break,
            }
        }
        names.reverse();
        names.join("/")
    }
}

impl AccountTree {
    pub fn new(max_nodes: usize) -> Self {
        Self(Arc::new(Mutex::new(Tree {
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
            max_nodes: max_nodes.max(1),
        })))
    }

    pub fn add_root(&self, name: &str, limit: u64) -> Result<NodeId, AccountError> {
        lock(&self.0).insert(Node {
            name: name.to_owned(),
            parent: None,
            limit,
            used: 0,
            children: 0,
        })
    }

    pub fn add_child(
        &self,
        parent: NodeId,
        name: &str,
        limit: u64,
    ) -> Result<NodeId, AccountError> {
        let mut t = lock(&self.0);
        t.get(parent)?; // validate before mutating
        let id = t.insert(Node {
            name: name.to_owned(),
            parent: Some(parent),
            limit,
            used: 0,
            children: 0,
        })?;
        t.get_mut(parent)?.children += 1;
        Ok(id)
    }

    /// Tightening below current use is allowed (existing charges stay;
    /// new ones are refused until usage falls).
    pub fn set_limit(&self, id: NodeId, limit: u64) -> Result<(), AccountError> {
        lock(&self.0).get_mut(id)?.limit = limit;
        Ok(())
    }

    pub fn used(&self, id: NodeId) -> Result<u64, AccountError> {
        Ok(lock(&self.0).get(id)?.used)
    }

    pub fn path(&self, id: NodeId) -> Result<String, AccountError> {
        let t = lock(&self.0);
        t.get(id)?;
        Ok(t.path(id))
    }

    /// Removes an idle leaf (no children, nothing charged) so per-peer /
    /// per-operation nodes don't accumulate forever.
    pub fn remove_if_idle(&self, id: NodeId) -> bool {
        let mut t = lock(&self.0);
        let Ok(n) = t.get(id) else { return false };
        if n.children != 0 || n.used != 0 {
            return false;
        }
        let parent = n.parent;
        let slot = &mut t.slots[id.index as usize];
        slot.node = None;
        slot.generation = slot.generation.wrapping_add(1);
        t.free.push(id.index);
        t.live -= 1;
        if let Some(p) = parent {
            if let Ok(pn) = t.get_mut(p) {
                pn.children = pn.children.saturating_sub(1);
            }
        }
        true
    }

    /// Charges `leaf` and every ancestor, or none of them.
    pub fn charge(&self, leaf: NodeId, amount: u64) -> Result<Charge, ChargeDenied> {
        let mut t = lock(&self.0);
        let unknown = || ChargeDenied {
            refused_by: "<unknown node>".into(),
            limit: 0,
            used: 0,
            requested: amount,
        };
        t.get(leaf).map_err(|_| unknown())?;
        // Pass 1: verify the whole chain.
        let mut cur = Some(leaf);
        while let Some(c) = cur {
            let n = t.get(c).map_err(|_| unknown())?;
            let fits = n.used.checked_add(amount).is_some_and(|u| u <= n.limit);
            if !fits {
                return Err(ChargeDenied {
                    refused_by: t.path(c),
                    limit: n.limit,
                    used: n.used,
                    requested: amount,
                });
            }
            cur = n.parent;
        }
        // Pass 2: commit.
        let mut cur = Some(leaf);
        while let Some(c) = cur {
            let n = t.get_mut(c).map_err(|_| unknown())?;
            n.used += amount; // cannot overflow: checked in pass 1
            cur = n.parent;
        }
        drop(t);
        Ok(Charge {
            tree: Arc::clone(&self.0),
            leaf,
            amount,
        })
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        let mut t = lock(&self.tree);
        let mut cur = Some(self.leaf);
        while let Some(c) = cur {
            match t.get_mut(c) {
                Ok(n) => {
                    n.used = n.used.saturating_sub(self.amount);
                    cur = n.parent;
                }
                Err(_) => break,
            }
        }
    }
}

// -------------------------------------------------------------- tenants

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TenantId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeClass {
    /// May use the tenant's own reserve first, then overflow to shared.
    Critical,
    /// Shared pool only; never any tenant's reserve.
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PoolError {
    #[error("reservations ({reserved}) exceed capacity ({capacity})")]
    OverReserved { reserved: u64, capacity: u64 },
    #[error("unknown tenant")]
    UnknownTenant,
    #[error("duplicate tenant in reservations")]
    DuplicateTenant,
    #[error("no room: needs {needed} from the shared pool, {available} available")]
    Exhausted { needed: u64, available: u64 },
}

#[derive(Debug)]
struct PoolState {
    shared_used: u64,
    reserve_used: HashMap<TenantId, u64>,
}

#[derive(Debug)]
struct PoolInner {
    shared_capacity: u64,
    reserved: HashMap<TenantId, u64>,
    state: Mutex<PoolState>,
}

#[derive(Debug, Clone)]
pub struct TenantPools(Arc<PoolInner>);

#[derive(Debug)]
#[must_use = "dropping the charge returns capacity to the pools"]
pub struct TenantCharge {
    pool: Arc<PoolInner>,
    tenant: TenantId,
    from_reserve: u64,
    from_shared: u64,
}

impl TenantPools {
    pub fn new(capacity: u64, reservations: &[(TenantId, u64)]) -> Result<Self, PoolError> {
        let mut reserved = HashMap::new();
        let mut sum = 0u64;
        for &(t, r) in reservations {
            if reserved.insert(t, r).is_some() {
                return Err(PoolError::DuplicateTenant);
            }
            sum = sum.saturating_add(r);
        }
        if sum > capacity {
            return Err(PoolError::OverReserved {
                reserved: sum,
                capacity,
            });
        }
        let reserve_used = reserved.keys().map(|t| (*t, 0)).collect();
        Ok(Self(Arc::new(PoolInner {
            shared_capacity: capacity - sum,
            reserved,
            state: Mutex::new(PoolState {
                shared_used: 0,
                reserve_used,
            }),
        })))
    }

    pub fn try_charge(
        &self,
        tenant: TenantId,
        class: ChargeClass,
        amount: u64,
    ) -> Result<TenantCharge, PoolError> {
        let reserve = *self
            .0
            .reserved
            .get(&tenant)
            .ok_or(PoolError::UnknownTenant)?;
        let mut st = lock(&self.0.state);
        let used_reserve = st.reserve_used.get(&tenant).copied().unwrap_or(0);
        let from_reserve = match class {
            ChargeClass::Critical => amount.min(reserve.saturating_sub(used_reserve)),
            ChargeClass::Normal => 0,
        };
        let from_shared = amount - from_reserve;
        let available = self.0.shared_capacity.saturating_sub(st.shared_used);
        if from_shared > available {
            return Err(PoolError::Exhausted {
                needed: from_shared,
                available,
            });
        }
        st.shared_used += from_shared;
        *st.reserve_used.entry(tenant).or_insert(0) += from_reserve;
        drop(st);
        Ok(TenantCharge {
            pool: Arc::clone(&self.0),
            tenant,
            from_reserve,
            from_shared,
        })
    }

    pub fn shared_used(&self) -> u64 {
        lock(&self.0.state).shared_used
    }

    pub fn reserve_used(&self, tenant: TenantId) -> u64 {
        lock(&self.0.state)
            .reserve_used
            .get(&tenant)
            .copied()
            .unwrap_or(0)
    }
}

impl Drop for TenantCharge {
    fn drop(&mut self) {
        let mut st = lock(&self.pool.state);
        st.shared_used = st.shared_used.saturating_sub(self.from_shared);
        if let Some(u) = st.reserve_used.get_mut(&self.tenant) {
            *u = u.saturating_sub(self.from_reserve);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files_bob(t: &AccountTree) -> (NodeId, NodeId, NodeId) {
        let files = t.add_root("files", 1_000).unwrap();
        let bob = t.add_child(files, "bob", 300).unwrap();
        let xfer = t.add_child(bob, "transfer-123", 200).unwrap();
        (files, bob, xfer)
    }

    #[test]
    fn charge_lands_on_every_ancestor_and_refunds_on_drop() {
        let t = AccountTree::new(16);
        let (files, bob, xfer) = files_bob(&t);
        let c = t.charge(xfer, 150).unwrap();
        assert_eq!(
            (
                t.used(files).unwrap(),
                t.used(bob).unwrap(),
                t.used(xfer).unwrap()
            ),
            (150, 150, 150)
        );
        drop(c);
        assert_eq!(
            (
                t.used(files).unwrap(),
                t.used(bob).unwrap(),
                t.used(xfer).unwrap()
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn denial_names_the_exact_node_and_changes_nothing() {
        let t = AccountTree::new(16);
        let (files, bob, xfer) = files_bob(&t);
        let _keep = t.charge(xfer, 150).unwrap();
        // Bob's own 300 limit: a sibling op wants 200 more (150+200 > 300)
        let other = t.add_child(bob, "transfer-456", 500).unwrap();
        let e = t.charge(other, 200).unwrap_err();
        assert_eq!(e.refused_by, "files/bob");
        assert_eq!((e.limit, e.used, e.requested), (300, 150, 200));
        assert_eq!(t.used(files).unwrap(), 150, "nothing partially applied");
        assert_eq!(t.used(other).unwrap(), 0);
    }

    #[test]
    fn leaf_limit_can_be_the_refuser() {
        let t = AccountTree::new(16);
        let (_, _, xfer) = files_bob(&t);
        assert_eq!(
            t.charge(xfer, 201).unwrap_err().refused_by,
            "files/bob/transfer-123"
        );
    }

    #[test]
    fn tightening_below_usage_keeps_charges_and_blocks_new_ones() {
        let t = AccountTree::new(16);
        let (_, bob, xfer) = files_bob(&t);
        let _c = t.charge(xfer, 100).unwrap();
        t.set_limit(bob, 50).unwrap();
        assert_eq!(t.used(bob).unwrap(), 100);
        assert!(t.charge(xfer, 1).is_err());
    }

    #[test]
    fn idle_nodes_are_removed_and_stale_ids_cannot_alias() {
        let t = AccountTree::new(16);
        let (_, bob, xfer) = files_bob(&t);
        let c = t.charge(xfer, 10).unwrap();
        assert!(!t.remove_if_idle(xfer), "in use");
        assert!(!t.remove_if_idle(bob), "has a child");
        drop(c);
        assert!(t.remove_if_idle(xfer));
        let new_leaf = t.add_child(bob, "transfer-999", 5).unwrap();
        assert_eq!(
            t.used(xfer),
            Err(AccountError::UnknownNode),
            "old id is dead even though slot reused"
        );
        assert_eq!(t.used(new_leaf), Ok(0));
        assert!(t.charge(xfer, 1).is_err());
    }

    #[test]
    fn tree_size_is_bounded_and_overflow_is_a_denial() {
        let t = AccountTree::new(2);
        let r = t.add_root("r", u64::MAX).unwrap();
        let c = t.add_child(r, "c", u64::MAX).unwrap();
        assert_eq!(t.add_child(r, "d", 1), Err(AccountError::TreeFull(2)));
        let _x = t.charge(c, u64::MAX).unwrap();
        assert!(t.charge(c, 1).is_err(), "u64 overflow handled");
    }

    #[test]
    fn tenant_a_cannot_touch_tenant_bs_reserve() {
        let (a, b) = (TenantId(1), TenantId(2));
        let p = TenantPools::new(100, &[(a, 20), (b, 30)]).unwrap(); // shared = 50
        let _fill = p.try_charge(a, ChargeClass::Normal, 50).unwrap();
        assert!(matches!(
            p.try_charge(a, ChargeClass::Normal, 1),
            Err(PoolError::Exhausted { .. })
        ));
        // B's critical work still has its full reserve despite A saturating shared.
        let bc = p.try_charge(b, ChargeClass::Critical, 30).unwrap();
        assert_eq!(p.reserve_used(b), 30);
        drop(bc);
        assert_eq!(p.reserve_used(b), 0);
    }

    #[test]
    fn critical_overflows_into_shared_but_normal_never_uses_reserve() {
        let (a, b) = (TenantId(1), TenantId(2));
        let p = TenantPools::new(100, &[(a, 20), (b, 30)]).unwrap();
        let c = p.try_charge(a, ChargeClass::Critical, 35).unwrap(); // 20 reserve + 15 shared
        assert_eq!((p.reserve_used(a), p.shared_used()), (20, 15));
        drop(c);
        assert_eq!((p.reserve_used(a), p.shared_used()), (0, 0));
        assert!(
            p.try_charge(a, ChargeClass::Normal, 51).is_err(),
            "reserve is not available to normal"
        );
    }

    #[test]
    fn pool_configuration_errors() {
        let a = TenantId(1);
        assert!(matches!(
            TenantPools::new(10, &[(a, 11)]),
            Err(PoolError::OverReserved { .. })
        ));
        assert_eq!(
            TenantPools::new(10, &[(a, 1), (a, 1)]).unwrap_err(),
            PoolError::DuplicateTenant
        );
        let p = TenantPools::new(10, &[(a, 1)]).unwrap();
        assert_eq!(
            p.try_charge(TenantId(9), ChargeClass::Normal, 1)
                .unwrap_err(),
            PoolError::UnknownTenant
        );
    }
}
