//! Merkle Tree implementation for audit log integrity.
//!
//! Binary Merkle tree using SHA256. Supports leaf insertion,
//! root hash computation, proof generation, and verification.

use sha2::{Digest, Sha256};

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
            return Err(format!(
                "index {} out of range (0..{})",
                leaf_index,
                self.leaves.len()
            ));
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
mod tests;
