package forge

import (
	"context"

	"github.com/276793422/NemesisBot/module/cluster"
)

// clusterForgeBridge adapts the cluster.Cluster to the ClusterForgeBridge interface.
// This lives in the forge package to avoid circular imports (forge → cluster is OK).
type clusterForgeBridge struct {
	cluster *cluster.Cluster
}

// NewClusterForgeBridge creates a bridge that connects Forge to the cluster subsystem.
func NewClusterForgeBridge(c *cluster.Cluster) ClusterForgeBridge {
	return &clusterForgeBridge{cluster: c}
}

func (b *clusterForgeBridge) ShareToPeer(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	return b.cluster.CallWithContext(ctx, peerID, action, payload)
}

func (b *clusterForgeBridge) GetOnlinePeers() []PeerInfo {
	rawPeers := b.cluster.GetOnlinePeers()
	result := make([]PeerInfo, 0, len(rawPeers))
	for _, p := range rawPeers {
		if node, ok := p.(*cluster.Node); ok {
			result = append(result, PeerInfo{
				ID:   node.GetID(),
				Name: node.GetName(),
			})
		}
	}
	return result
}

func (b *clusterForgeBridge) IsClusterEnabled() bool {
	return b.cluster.IsRunning()
}
