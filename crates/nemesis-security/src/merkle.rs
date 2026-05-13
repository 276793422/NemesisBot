//! Merkle Tree implementation for audit log integrity.
//!
//! Binary Merkle tree using SHA256. Supports leaf insertion,
//! root hash computation, proof generation, and verification.

use sha2::{Sha256, Digest};

/// Hex-encoded SHA256 digest.
pub type Hash = String;

/// Compute SHA256 of arbitrary data and return hex-encoded hash.
pub fn sha256_hex(data: &[u8]) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Compute SHA256 of the concatenation of two hex-encoded hashes.
pub fn sha256_pair_hex(a: &str, b: &str) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(a.as_bytes());
    hasher.update(b.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// One step in a Merkle proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofStep {
    /// Sibling hash.
    pub hash: Hash,
    /// Direction: "left" means the proof hash is on the left (prepend it).
    pub direction: String,
}

/// Binary Merkle tree using SHA256.
///
/// Leaves are added sequentially. The tree is rebuilt from scratch on every
/// insertion so that the root hash is always current.
///
/// The tree is NOT thread-safe; callers must synchronize access.
#[derive(Debug, Clone)]
pub struct MerkleTree {
    leaves: Vec<Hash>,
    root: Hash,
}

impl MerkleTree {
    /// Create an empty Merkle tree.
    pub fn new() -> Self {
        Self {
            leaves: Vec::new(),
            root: sha256_hex(&[]),
        }
    }

    /// Append data as a new leaf node and return its hash.
    /// The tree root is recomputed.
    pub fn add_leaf(&mut self, data: &[u8]) -> Hash {
        let leaf_hash = sha256_hex(data);
        self.leaves.push(leaf_hash.clone());
        self.rebuild_root();
        leaf_hash
    }

    /// Get the current root hash.
    pub fn root_hash(&self) -> &Hash {
        &self.root
    }

    /// Get the number of leaves.
    pub fn size(&self) -> usize {
        self.leaves.len()
    }

    /// Get a copy of all leaf hashes.
    pub fn leaves(&self) -> Vec<Hash> {
        self.leaves.clone()
    }

    /// Generate a Merkle inclusion proof for the leaf at `leaf_index`.
    ///
    /// Returns a list of sibling hashes ordered from the leaf level up toward
    /// the root, each with a direction flag.
    pub fn proof(&self, leaf_index: usize) -> Result<Vec<ProofStep>, String> {
        if leaf_index >= self.leaves.len() {
            return Err(format!("index {} out of range (0..{})", leaf_index, self.leaves.len()));
        }
        if self.leaves.len() == 1 {
            return Ok(vec![]);
        }
        self.build_proof(leaf_index)
    }

    /// Verify that `leaf_data` exists in the tree by recomputing the root
    /// from the leaf hash and the supplied proof steps.
    pub fn verify(&self, leaf_data: &[u8], proof: &[ProofStep]) -> bool {
        let hash = sha256_hex(leaf_data);
        Self::verify_from_hash(&hash, proof, &self.root)
    }

    /// Verify a leaf hash against the root using proof steps.
    pub fn verify_from_hash(leaf_hash: &str, proof: &[ProofStep], root: &str) -> bool {
        let mut current = leaf_hash.to_string();
        for step in proof {
            match step.direction.as_str() {
                "left" => current = sha256_pair_hex(&step.hash, &current),
                "right" => current = sha256_pair_hex(&current, &step.hash),
                _ => return false,
            }
        }
        current == root
    }

    fn rebuild_root(&mut self) {
        if self.leaves.is_empty() {
            self.root = sha256_hex(&[]);
            return;
        }
        let mut level = self.leaves.clone();
        while level.len() > 1 {
            let mut next = Vec::with_capacity((level.len() + 1) / 2);
            let mut i = 0;
            while i < level.len() {
                if i + 1 < level.len() {
                    next.push(sha256_pair_hex(&level[i], &level[i + 1]));
                    i += 2;
                } else {
                    next.push(level[i].clone());
                    i += 1;
                }
            }
            level = next;
        }
        self.root = level.remove(0);
    }

    fn build_proof(&self, leaf_index: usize) -> Result<Vec<ProofStep>, String> {
        let mut steps = Vec::new();
        let mut level = self.leaves.clone();
        let mut idx = leaf_index;

        while level.len() > 1 {
            let mut next = Vec::new();
            let mut next_idx = 0usize;
            let mut i = 0;
            while i < level.len() {
                if i + 1 < level.len() {
                    if i == idx {
                        steps.push(ProofStep {
                            hash: level[i + 1].clone(),
                            direction: "right".to_string(),
                        });
                        next_idx = next.len();
                    } else if i + 1 == idx {
                        steps.push(ProofStep {
                            hash: level[i].clone(),
                            direction: "left".to_string(),
                        });
                        next_idx = next.len();
                    }
                    next.push(sha256_pair_hex(&level[i], &level[i + 1]));
                    i += 2;
                } else {
                    if i == idx {
                        next_idx = next.len();
                    }
                    next.push(level[i].clone());
                    i += 1;
                }
            }
            level = next;
            idx = next_idx;
        }
        Ok(steps)
    }
}

impl Default for MerkleTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_tree() {
        let tree = MerkleTree::new();
        assert_eq!(tree.size(), 0);
        assert_ne!(tree.root_hash(), "");
    }

    #[test]
    fn test_single_leaf() {
        let mut tree = MerkleTree::new();
        let hash = tree.add_leaf(b"hello");
        assert_eq!(tree.size(), 1);
        assert_eq!(*tree.root_hash(), hash);
    }

    #[test]
    fn test_two_leaves() {
        let mut tree = MerkleTree::new();
        let h1 = tree.add_leaf(b"left");
        let h2 = tree.add_leaf(b"right");
        assert_eq!(tree.size(), 2);
        assert_eq!(*tree.root_hash(), sha256_pair_hex(&h1, &h2));
    }

    #[test]
    fn test_three_leaves() {
        let mut tree = MerkleTree::new();
        tree.add_leaf(b"a");
        tree.add_leaf(b"b");
        tree.add_leaf(b"c");
        assert_eq!(tree.size(), 3);
    }

    #[test]
    fn test_proof_and_verify() {
        let mut tree = MerkleTree::new();
        tree.add_leaf(b"leaf0");
        tree.add_leaf(b"leaf1");
        tree.add_leaf(b"leaf2");
        tree.add_leaf(b"leaf3");

        for i in 0..4 {
            let data = format!("leaf{}", i);
            let proof = tree.proof(i).unwrap();
            assert!(tree.verify(data.as_bytes(), &proof), "proof for leaf {} should verify", i);
        }
    }

    #[test]
    fn test_verify_from_hash() {
        let mut tree = MerkleTree::new();
        let h0 = tree.add_leaf(b"data0");
        tree.add_leaf(b"data1");
        tree.add_leaf(b"data2");

        let proof = tree.proof(0).unwrap();
        assert!(MerkleTree::verify_from_hash(&h0, &proof, tree.root_hash()));
    }

    #[test]
    fn test_proof_out_of_range() {
        let mut tree = MerkleTree::new();
        tree.add_leaf(b"x");
        assert!(tree.proof(1).is_err());
    }

    #[test]
    fn test_proof_single_leaf_empty() {
        let mut tree = MerkleTree::new();
        tree.add_leaf(b"only");
        let proof = tree.proof(0).unwrap();
        assert!(proof.is_empty());
    }

    #[test]
    fn test_tampered_proof_fails() {
        let mut tree = MerkleTree::new();
        tree.add_leaf(b"a");
        tree.add_leaf(b"b");
        tree.add_leaf(b"c");

        let proof = tree.proof(0).unwrap();
        // Verify with wrong data should fail
        assert!(!tree.verify(b"wrong_data", &proof));
    }

    // ---- Additional Merkle tree tests ----

    #[test]
    fn test_sha256_hex_empty() {
        let hash = sha256_hex(&[]);
        assert_eq!(hash.len(), 64); // SHA256 = 32 bytes = 64 hex chars
    }

    #[test]
    fn test_sha256_hex_known() {
        // SHA256("hello") is well-known
        let hash = sha256_hex(b"hello");
        assert_eq!(hash, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn test_sha256_hex_deterministic() {
        let h1 = sha256_hex(b"test data");
        let h2 = sha256_hex(b"test data");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_sha256_pair_hex_order_matters() {
        let a = sha256_hex(b"left");
        let b = sha256_hex(b"right");
        let h1 = sha256_pair_hex(&a, &b);
        let h2 = sha256_pair_hex(&b, &a);
        assert_ne!(h1, h2, "hash(a,b) should differ from hash(b,a)");
    }

    #[test]
    fn test_proof_step_equality() {
        let s1 = ProofStep { hash: "abc".to_string(), direction: "left".to_string() };
        let s2 = ProofStep { hash: "abc".to_string(), direction: "left".to_string() };
        let s3 = ProofStep { hash: "abc".to_string(), direction: "right".to_string() };
        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_default_impl() {
        let tree = MerkleTree::default();
        assert_eq!(tree.size(), 0);
        assert_ne!(tree.root_hash(), "");
    }

    #[test]
    fn test_add_leaf_returns_correct_hash() {
        let mut tree = MerkleTree::new();
        let hash = tree.add_leaf(b"test");
        assert_eq!(hash, sha256_hex(b"test"));
    }

    #[test]
    fn test_root_hash_changes_on_insert() {
        let mut tree = MerkleTree::new();
        let root0 = tree.root_hash().clone();
        tree.add_leaf(b"first");
        let root1 = tree.root_hash().clone();
        assert_ne!(root0, root1);
        tree.add_leaf(b"second");
        let root2 = tree.root_hash().clone();
        assert_ne!(root1, root2);
    }

    #[test]
    fn test_four_leaves_root_matches_manual() {
        let mut tree = MerkleTree::new();
        let h0 = tree.add_leaf(b"a");
        let h1 = tree.add_leaf(b"b");
        let h2 = tree.add_leaf(b"c");
        let h3 = tree.add_leaf(b"d");

        let expected_root = sha256_pair_hex(
            &sha256_pair_hex(&h0, &h1),
            &sha256_pair_hex(&h2, &h3),
        );
        assert_eq!(*tree.root_hash(), expected_root);
    }

    #[test]
    fn test_five_leaves() {
        let mut tree = MerkleTree::new();
        for i in 0..5 {
            tree.add_leaf(format!("leaf{}", i).as_bytes());
        }
        assert_eq!(tree.size(), 5);
        // Root should be non-empty
        assert!(!tree.root_hash().is_empty());
    }

    #[test]
    fn test_leaves_accessor() {
        let mut tree = MerkleTree::new();
        tree.add_leaf(b"x");
        tree.add_leaf(b"y");
        let leaves = tree.leaves();
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0], sha256_hex(b"x"));
        assert_eq!(leaves[1], sha256_hex(b"y"));
    }

    #[test]
    fn test_proof_two_leaves() {
        let mut tree = MerkleTree::new();
        tree.add_leaf(b"left");
        tree.add_leaf(b"right");

        let proof_left = tree.proof(0).unwrap();
        assert_eq!(proof_left.len(), 1);
        assert_eq!(proof_left[0].direction, "right");

        let proof_right = tree.proof(1).unwrap();
        assert_eq!(proof_right.len(), 1);
        assert_eq!(proof_right[0].direction, "left");
    }

    #[test]
    fn test_proof_eight_leaves_all_verify() {
        let mut tree = MerkleTree::new();
        for i in 0..8 {
            tree.add_leaf(format!("leaf{}", i).as_bytes());
        }

        for i in 0..8 {
            let data = format!("leaf{}", i);
            let proof = tree.proof(i).unwrap();
            assert!(tree.verify(data.as_bytes(), &proof),
                "proof for leaf {} should verify", i);
        }
    }

    #[test]
    fn test_verify_from_hash_wrong_root() {
        let mut tree = MerkleTree::new();
        let h0 = tree.add_leaf(b"data0");
        tree.add_leaf(b"data1");
        let proof = tree.proof(0).unwrap();
        assert!(!MerkleTree::verify_from_hash(&h0, &proof, "wrong_root_hash"));
    }

    #[test]
    fn test_verify_from_hash_invalid_direction() {
        let mut tree = MerkleTree::new();
        let h0 = tree.add_leaf(b"data0");
        tree.add_leaf(b"data1");
        let mut proof = tree.proof(0).unwrap();
        // Tamper with direction
        proof[0].direction = "invalid".to_string();
        assert!(!MerkleTree::verify_from_hash(&h0, &proof, tree.root_hash()));
    }

    #[test]
    fn test_proof_out_of_range_various() {
        let mut tree = MerkleTree::new();
        tree.add_leaf(b"a");
        tree.add_leaf(b"b");
        assert!(tree.proof(2).is_err());
        assert!(tree.proof(100).is_err());
    }

    #[test]
    fn test_empty_tree_proof_out_of_range() {
        let tree = MerkleTree::new();
        assert!(tree.proof(0).is_err());
    }

    #[test]
    fn test_large_tree_consistency() {
        let mut tree = MerkleTree::new();
        let mut hashes = Vec::new();
        for i in 0..32 {
            let h = tree.add_leaf(format!("item{}", i).as_bytes());
            hashes.push(h);
        }
        assert_eq!(tree.size(), 32);
        // Verify a few random proofs
        for i in [0, 7, 15, 31] {
            let data = format!("item{}", i);
            let proof = tree.proof(i).unwrap();
            assert!(tree.verify(data.as_bytes(), &proof));
        }
    }

    #[test]
    fn test_same_data_different_positions() {
        let mut tree = MerkleTree::new();
        tree.add_leaf(b"same");
        tree.add_leaf(b"same");
        tree.add_leaf(b"same");

        // All proofs should verify
        for i in 0..3 {
            let proof = tree.proof(i).unwrap();
            assert!(tree.verify(b"same", &proof), "proof for index {} should verify", i);
        }
    }
}
