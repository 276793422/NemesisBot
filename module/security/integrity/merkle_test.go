package integrity_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/security/integrity"
)

// ---------------------------------------------------------------------------
// NewMerkleTree
// ---------------------------------------------------------------------------

func TestNewMerkleTree(t *testing.T) {
	tree := integrity.NewMerkleTree()
	if tree == nil {
		t.Fatal("expected non-nil tree")
	}
	if tree.Size() != 0 {
		t.Errorf("expected Size()=0, got %d", tree.Size())
	}
}

// ---------------------------------------------------------------------------
// RootHash — empty tree
// ---------------------------------------------------------------------------

func TestMerkleTree_RootHash_Empty(t *testing.T) {
	tree := integrity.NewMerkleTree()
	root := tree.RootHash()
	if root == "" {
		t.Error("expected non-empty root hash for empty tree")
	}
	// Empty tree root should be SHA256 of nil.
	if len(root) != 64 {
		t.Errorf("expected 64-char hex hash, got %d chars", len(root))
	}
}

// ---------------------------------------------------------------------------
// AddLeaf and RootHash — single leaf
// ---------------------------------------------------------------------------

func TestMerkleTree_SingleLeaf(t *testing.T) {
	tree := integrity.NewMerkleTree()
	leafHash := tree.AddLeaf([]byte("hello"))
	if leafHash == "" {
		t.Error("expected non-empty leaf hash")
	}
	if tree.Size() != 1 {
		t.Errorf("expected Size()=1, got %d", tree.Size())
	}
	root := tree.RootHash()
	if root == "" {
		t.Error("expected non-empty root hash")
	}
	// With one leaf, root should equal the leaf hash.
	if root != leafHash {
		t.Errorf("expected root == leaf hash for single leaf: root=%s leaf=%s", root, leafHash)
	}
}

// ---------------------------------------------------------------------------
// AddLeaf — two leaves
// ---------------------------------------------------------------------------

func TestMerkleTree_TwoLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	h1 := tree.AddLeaf([]byte("leaf1"))
	h2 := tree.AddLeaf([]byte("leaf2"))

	if tree.Size() != 2 {
		t.Errorf("expected Size()=2, got %d", tree.Size())
	}

	root := tree.RootHash()
	if root == "" {
		t.Error("expected non-empty root hash")
	}

	// Verify root is not the same as individual leaves.
	if root == h1 || root == h2 {
		t.Error("root should be a combination of both leaves")
	}
}

// ---------------------------------------------------------------------------
// AddLeaf — three leaves
// ---------------------------------------------------------------------------

func TestMerkleTree_ThreeLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("a"))
	tree.AddLeaf([]byte("b"))
	tree.AddLeaf([]byte("c"))

	if tree.Size() != 3 {
		t.Errorf("expected Size()=3, got %d", tree.Size())
	}
	root := tree.RootHash()
	if root == "" {
		t.Error("expected non-empty root hash")
	}
}

// ---------------------------------------------------------------------------
// AddLeaf — four leaves (power of 2)
// ---------------------------------------------------------------------------

func TestMerkleTree_FourLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("a"))
	tree.AddLeaf([]byte("b"))
	tree.AddLeaf([]byte("c"))
	tree.AddLeaf([]byte("d"))

	if tree.Size() != 4 {
		t.Errorf("expected Size()=4, got %d", tree.Size())
	}
	root := tree.RootHash()
	if root == "" {
		t.Error("expected non-empty root hash")
	}
}

// ---------------------------------------------------------------------------
// AddLeaf — five leaves
// ---------------------------------------------------------------------------

func TestMerkleTree_FiveLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	for i := 0; i < 5; i++ {
		tree.AddLeaf([]byte{byte(i)})
	}
	if tree.Size() != 5 {
		t.Errorf("expected Size()=5, got %d", tree.Size())
	}
}

// ---------------------------------------------------------------------------
// AddLeaf — many leaves
// ---------------------------------------------------------------------------

func TestMerkleTree_ManyLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	for i := 0; i < 100; i++ {
		tree.AddLeaf([]byte{byte(i)})
	}
	if tree.Size() != 100 {
		t.Errorf("expected Size()=100, got %d", tree.Size())
	}
	root := tree.RootHash()
	if root == "" {
		t.Error("expected non-empty root hash")
	}
}

// ---------------------------------------------------------------------------
// AddLeaf — same data produces same hash
// ---------------------------------------------------------------------------

func TestMerkleTree_SameData(t *testing.T) {
	tree1 := integrity.NewMerkleTree()
	tree2 := integrity.NewMerkleTree()

	tree1.AddLeaf([]byte("data"))
	tree2.AddLeaf([]byte("data"))

	if tree1.RootHash() != tree2.RootHash() {
		t.Error("same data should produce same root hash")
	}
}

// ---------------------------------------------------------------------------
// AddLeaf — different data produces different hash
// ---------------------------------------------------------------------------

func TestMerkleTree_DifferentData(t *testing.T) {
	tree1 := integrity.NewMerkleTree()
	tree2 := integrity.NewMerkleTree()

	tree1.AddLeaf([]byte("data1"))
	tree2.AddLeaf([]byte("data2"))

	if tree1.RootHash() == tree2.RootHash() {
		t.Error("different data should produce different root hash")
	}
}

// ---------------------------------------------------------------------------
// Leaves
// ---------------------------------------------------------------------------

func TestMerkleTree_Leaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("a"))
	tree.AddLeaf([]byte("b"))

	leaves := tree.Leaves()
	if len(leaves) != 2 {
		t.Fatalf("expected 2 leaves, got %d", len(leaves))
	}

	// Verify the returned slice is a copy.
	leaves[0] = "tampered"
	original := tree.Leaves()
	if original[0] == "tampered" {
		t.Error("Leaves() should return a copy")
	}
}

// ---------------------------------------------------------------------------
// Proof — single leaf
// ---------------------------------------------------------------------------

func TestMerkleTree_Proof_SingleLeaf(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("only"))

	proof, err := tree.Proof(0)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(proof) != 0 {
		t.Errorf("expected empty proof for single leaf, got %d steps", len(proof))
	}
}

// ---------------------------------------------------------------------------
// Proof — two leaves
// ---------------------------------------------------------------------------

func TestMerkleTree_Proof_TwoLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("left"))
	tree.AddLeaf([]byte("right"))

	proof0, err := tree.Proof(0)
	if err != nil {
		t.Fatalf("unexpected error for proof(0): %v", err)
	}
	if len(proof0) != 1 {
		t.Fatalf("expected 1 proof step, got %d", len(proof0))
	}
	if proof0[0].Direction != "right" {
		t.Errorf("expected direction 'right' for left leaf sibling, got %q", proof0[0].Direction)
	}

	proof1, err := tree.Proof(1)
	if err != nil {
		t.Fatalf("unexpected error for proof(1): %v", err)
	}
	if len(proof1) != 1 {
		t.Fatalf("expected 1 proof step, got %d", len(proof1))
	}
	if proof1[0].Direction != "left" {
		t.Errorf("expected direction 'left' for right leaf sibling, got %q", proof1[0].Direction)
	}
}

// ---------------------------------------------------------------------------
// Proof — out of range
// ---------------------------------------------------------------------------

func TestMerkleTree_Proof_OutOfRange(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("a"))

	_, err := tree.Proof(-1)
	if err == nil {
		t.Error("expected error for negative index")
	}

	_, err = tree.Proof(1)
	if err == nil {
		t.Error("expected error for index out of range")
	}
}

// ---------------------------------------------------------------------------
// Proof — four leaves
// ---------------------------------------------------------------------------

func TestMerkleTree_Proof_FourLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("a"))
	tree.AddLeaf([]byte("b"))
	tree.AddLeaf([]byte("c"))
	tree.AddLeaf([]byte("d"))

	for i := 0; i < 4; i++ {
		proof, err := tree.Proof(i)
		if err != nil {
			t.Fatalf("Proof(%d): unexpected error: %v", i, err)
		}
		if len(proof) != 2 {
			t.Errorf("Proof(%d): expected 2 steps, got %d", i, len(proof))
		}
	}
}

// ---------------------------------------------------------------------------
// Verify
// ---------------------------------------------------------------------------

func TestMerkleTree_Verify_SingleLeaf(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("only"))

	proof, _ := tree.Proof(0)
	if !tree.Verify([]byte("only"), proof) {
		t.Error("expected verification to succeed for single leaf")
	}
}

func TestMerkleTree_Verify_TwoLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("left"))
	tree.AddLeaf([]byte("right"))

	for _, leaf := range []string{"left", "right"} {
		idx := 0
		if leaf == "right" {
			idx = 1
		}
		proof, _ := tree.Proof(idx)
		if !tree.Verify([]byte(leaf), proof) {
			t.Errorf("expected verification to succeed for leaf %q", leaf)
		}
	}
}

func TestMerkleTree_Verify_FourLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	data := []string{"a", "b", "c", "d"}
	for _, d := range data {
		tree.AddLeaf([]byte(d))
	}

	for i, d := range data {
		proof, _ := tree.Proof(i)
		if !tree.Verify([]byte(d), proof) {
			t.Errorf("expected verification to succeed for leaf %q at index %d", d, i)
		}
	}
}

func TestMerkleTree_Verify_ThreeLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	data := []string{"a", "b", "c"}
	for _, d := range data {
		tree.AddLeaf([]byte(d))
	}

	for i, d := range data {
		proof, _ := tree.Proof(i)
		if !tree.Verify([]byte(d), proof) {
			t.Errorf("expected verification to succeed for leaf %q at index %d", d, i)
		}
	}
}

func TestMerkleTree_Verify_WrongData(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("correct"))
	tree.AddLeaf([]byte("other"))

	proof, _ := tree.Proof(0)
	if tree.Verify([]byte("wrong"), proof) {
		t.Error("expected verification to fail for wrong data")
	}
}

func TestMerkleTree_Verify_InvalidDirection(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("a"))
	tree.AddLeaf([]byte("b"))

	proof, _ := tree.Proof(0)
	// Tamper with direction.
	proof[0].Direction = "invalid"
	if tree.Verify([]byte("a"), proof) {
		t.Error("expected verification to fail for invalid direction")
	}
}

// ---------------------------------------------------------------------------
// VerifyFromHash
// ---------------------------------------------------------------------------

func TestVerifyFromHash_Valid(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("data"))

	proof, _ := tree.Proof(0)
	leafHash := tree.Leaves()[0]
	if !integrity.VerifyFromHash(leafHash, proof, tree.RootHash()) {
		t.Error("expected VerifyFromHash to succeed")
	}
}

func TestVerifyFromHash_InvalidHash(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("data"))

	proof, _ := tree.Proof(0)
	if integrity.VerifyFromHash("invalidhash", proof, tree.RootHash()) {
		t.Error("expected VerifyFromHash to fail for invalid hash")
	}
}

// ---------------------------------------------------------------------------
// Deterministic root hash
// ---------------------------------------------------------------------------

func TestMerkleTree_Deterministic(t *testing.T) {
	tree1 := integrity.NewMerkleTree()
	tree2 := integrity.NewMerkleTree()

	data := []string{"x", "y", "z", "w"}
	for _, d := range data {
		tree1.AddLeaf([]byte(d))
		tree2.AddLeaf([]byte(d))
	}

	if tree1.RootHash() != tree2.RootHash() {
		t.Error("identical data should produce identical root hashes")
	}
}

// ---------------------------------------------------------------------------
// Proof for odd number of leaves (5, 7)
// ---------------------------------------------------------------------------

func TestMerkleTree_Proof_OddLeaves(t *testing.T) {
	for _, count := range []int{5, 7, 9} {
		t.Run(string(rune('0'+count)), func(t *testing.T) {
			tree := integrity.NewMerkleTree()
			for i := 0; i < count; i++ {
				tree.AddLeaf([]byte{byte(i)})
			}

			for i := 0; i < count; i++ {
				proof, err := tree.Proof(i)
				if err != nil {
					t.Fatalf("Proof(%d) error: %v", i, err)
				}
				if !tree.Verify([]byte{byte(i)}, proof) {
					t.Errorf("Verify failed for leaf %d with %d leaves", i, count)
				}
			}
		})
	}
}
