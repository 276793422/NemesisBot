// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package integrity provides a Merkle hash tree for audit log integrity
package integrity

import (
	"crypto/sha256"
	"encoding/hex"
)

// Hash is a hex-encoded SHA256 digest string.
type Hash = string

// emptyHash is the SHA256 of no input bytes.
var emptyHash = sha256Hex(nil)

// sha256Hex computes SHA256 and returns the hex-encoded digest.
func sha256Hex(data []byte) Hash {
	h := sha256.Sum256(data)
	return hex.EncodeToString(h[:])
}

// sha256PairHex computes SHA256 of the concatenation of two hex-encoded hashes.
func sha256PairHex(a, b Hash) Hash {
	h := sha256.Sum256([]byte(a + b))
	return hex.EncodeToString(h[:])
}

// MerkleTree implements a simple binary Merkle tree using SHA256.
// Leaves are added sequentially.  The tree is rebuilt from scratch on
// every insertion so that the root hash is always current.
//
// The tree is NOT thread-safe; callers must synchronize access.
type MerkleTree struct {
	leaves []Hash // leaf hashes in insertion order
	root   Hash   // cached root hash
}

// NewMerkleTree creates an empty Merkle tree.
func NewMerkleTree() *MerkleTree {
	return &MerkleTree{
		leaves: make([]Hash, 0),
		root:   emptyHash,
	}
}

// AddLeaf appends data as a new leaf node and returns its hash.
// The tree root is recomputed.
func (t *MerkleTree) AddLeaf(data []byte) Hash {
	leafHash := sha256Hex(data)
	t.leaves = append(t.leaves, leafHash)
	t.rebuildRoot()
	return leafHash
}

// RootHash returns the current root hash of the tree.
// For an empty tree this returns the SHA256 of an empty byte slice.
func (t *MerkleTree) RootHash() Hash {
	return t.root
}

// Size returns the number of leaves in the tree.
func (t *MerkleTree) Size() int {
	return len(t.leaves)
}

// Leaves returns a copy of all leaf hashes.
func (t *MerkleTree) Leaves() []Hash {
	out := make([]Hash, len(t.leaves))
	copy(out, t.leaves)
	return out
}

// Proof generates a Merkle inclusion proof for the leaf at leafIndex.
// The proof is the list of sibling hashes needed to reconstruct the root,
// ordered from the leaf level up toward the root.  Each entry is accompanied
// by a direction flag: "left" means the proof hash is on the left (prepend
// it), "right" means the proof hash is on the right (append it).
//
// Verification recomputes: hash = H(proof[i].Hash + hash) or
// hash = H(hash + proof[i].Hash) depending on the direction.
func (t *MerkleTree) Proof(leafIndex int) ([]ProofStep, error) {
	if leafIndex < 0 || leafIndex >= len(t.leaves) {
		return nil, ErrIndexOutOfRange
	}

	if len(t.leaves) == 1 {
		// Single leaf: proof is empty (leaf is the root).
		return []ProofStep{}, nil
	}

	// Build the full tree level by level, tracking the index.
	return t.buildProof(leafIndex)
}

// ProofStep represents one step in a Merkle proof.
type ProofStep struct {
	Hash      Hash   // sibling hash
	Direction string // "left" or "right"
}

// Verify checks that leafData exists in the tree by recomputing the root
// from the leaf hash and the supplied proof steps.
func (t *MerkleTree) Verify(leafData []byte, proof []ProofStep) bool {
	hash := sha256Hex(leafData)
	return verifyFromHash(hash, proof, t.root)
}

// VerifyFromHash checks that a leaf with the given hash exists in the tree.
func VerifyFromHash(leafHash Hash, proof []ProofStep, root Hash) bool {
	return verifyFromHash(leafHash, proof, root)
}

func verifyFromHash(leafHash Hash, proof []ProofStep, root Hash) bool {
	current := leafHash
	for _, step := range proof {
		switch step.Direction {
		case "left":
			current = sha256PairHex(step.Hash, current)
		case "right":
			current = sha256PairHex(current, step.Hash)
		default:
			return false
		}
	}
	return current == root
}

// rebuildRoot recomputes the root hash from the current leaves.
func (t *MerkleTree) rebuildRoot() {
	if len(t.leaves) == 0 {
		t.root = emptyHash
		return
	}

	level := make([]Hash, len(t.leaves))
	copy(level, t.leaves)

	for len(level) > 1 {
		var next []Hash
		for i := 0; i < len(level); i += 2 {
			if i+1 < len(level) {
				next = append(next, sha256PairHex(level[i], level[i+1]))
			} else {
				// Odd node: promote to next level unpaired.
				next = append(next, level[i])
			}
		}
		level = next
	}

	t.root = level[0]
}

// buildProof constructs the Merkle proof for the given leaf index.
func (t *MerkleTree) buildProof(leafIndex int) ([]ProofStep, error) {
	var steps []ProofStep

	level := make([]Hash, len(t.leaves))
	copy(level, t.leaves)

	idx := leafIndex
	for len(level) > 1 {
		var next []Hash
		var nextIdx int

		for i := 0; i < len(level); i += 2 {
			if i+1 < len(level) {
				if i == idx {
					// Current node; sibling is on the right.
					steps = append(steps, ProofStep{
						Hash:      level[i+1],
						Direction: "right",
					})
					nextIdx = len(next)
				} else if i+1 == idx {
					// Current node; sibling is on the left.
					steps = append(steps, ProofStep{
						Hash:      level[i],
						Direction: "left",
					})
					nextIdx = len(next)
				}
				next = append(next, sha256PairHex(level[i], level[i+1]))
			} else {
				// Odd node promoted.
				if i == idx {
					nextIdx = len(next)
				}
				next = append(next, level[i])
			}
		}
		level = next
		idx = nextIdx
	}

	return steps, nil
}
