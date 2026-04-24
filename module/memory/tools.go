// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package memory

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/memory/episodic"
	"github.com/276793422/NemesisBot/module/memory/graph"
	"github.com/276793422/NemesisBot/module/tools"
)

// EpisodicStore defines the interface for episodic memory operations needed by tools.
type EpisodicStore interface {
	StoreEpisode(ctx context.Context, episode *episodic.Episode) error
	GetRecent(ctx context.Context, sessionKey string, limit int) ([]*episodic.Episode, error)
	Search(ctx context.Context, query string, limit int) ([]*episodic.Episode, error)
	DeleteSession(ctx context.Context, sessionKey string) error
	Cleanup(ctx context.Context, olderThan time.Duration) (int, error)
}

// GraphStore defines the interface for knowledge graph operations needed by tools.
type GraphStore interface {
	AddEntity(ctx context.Context, entity *graph.Entity) error
	GetEntity(ctx context.Context, name string) (*graph.Entity, error)
	AddTriple(ctx context.Context, triple *graph.Triple) error
	Query(ctx context.Context, subject, predicate, object string) ([]*graph.Triple, error)
	GetRelated(ctx context.Context, entityName string, depth int) ([]*graph.Triple, error)
	Search(ctx context.Context, query string, limit int) ([]*graph.Triple, error)
	DeleteEntity(ctx context.Context, name string) error
}

// StoreProvider provides access to the episodic and graph memory stores.
// The Manager type will implement this interface.
type StoreProvider interface {
	GetEpisodicStore() EpisodicStore
	GetGraphStore() GraphStore
}

// NewMemoryTools creates all memory tools for registration with the tool registry.
func NewMemoryTools(sp StoreProvider) []tools.Tool {
	return []tools.Tool{
		NewMemorySearchTool(sp),
		NewMemoryStoreTool(sp),
		NewMemoryForgetTool(sp),
		NewMemoryListTool(sp),
	}
}

// --- memory_search ---

// MemorySearchTool searches episodic memories and knowledge graph.
type MemorySearchTool struct {
	sp StoreProvider
}

// NewMemorySearchTool creates a new memory search tool.
func NewMemorySearchTool(sp StoreProvider) *MemorySearchTool {
	return &MemorySearchTool{sp: sp}
}

func (t *MemorySearchTool) Name() string {
	return "memory_search"
}

func (t *MemorySearchTool) Description() string {
	return "Search stored memories. Can search episodic memories (past conversations) or knowledge graph (entities and relationships)."
}

func (t *MemorySearchTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"query": map[string]interface{}{
				"type":        "string",
				"description": "Search query text",
			},
			"memory_type": map[string]interface{}{
				"type":        "string",
				"description": "Type of memory to search: 'episodic', 'graph', or 'all'",
				"default":     "all",
			},
			"limit": map[string]interface{}{
				"type":        "integer",
				"description": "Maximum number of results (1-50)",
				"minimum":     1.0,
				"maximum":     50.0,
				"default":     10.0,
			},
		},
		"required": []string{"query"},
	}
}

func (t *MemorySearchTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	query, _ := args["query"].(string)
	if query == "" {
		return tools.ErrorResult("query is required")
	}

	memoryType, _ := args["memory_type"].(string)
	if memoryType == "" {
		memoryType = "all"
	}

	limit := 10
	if l, ok := args["limit"].(float64); ok {
		if int(l) > 0 && int(l) <= 50 {
			limit = int(l)
		}
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Memory search results for: %s\n\n", query))

	// Search episodic memories
	if memoryType == "all" || memoryType == "episodic" {
		epStore := t.sp.GetEpisodicStore()
		if epStore != nil {
			episodes, err := epStore.Search(ctx, query, limit)
			if err != nil {
				sb.WriteString(fmt.Sprintf("Episodic search error: %v\n", err))
			} else if len(episodes) == 0 {
				sb.WriteString("No episodic memories found.\n")
			} else {
				sb.WriteString(fmt.Sprintf("### Episodic Memories (%d results)\n\n", len(episodes)))
				for i, ep := range episodes {
					sb.WriteString(fmt.Sprintf("%d. [%s] %s: %s\n",
						i+1,
						ep.Timestamp.Format("2006-01-02 15:04"),
						ep.Role,
						truncateText(ep.Content, 200)))
					if len(ep.Tags) > 0 {
						sb.WriteString(fmt.Sprintf("   Tags: %s\n", strings.Join(ep.Tags, ", ")))
					}
				}
				sb.WriteString("\n")
			}
		}
	}

	// Search knowledge graph
	if memoryType == "all" || memoryType == "graph" {
		gStore := t.sp.GetGraphStore()
		if gStore != nil {
			triples, err := gStore.Search(ctx, query, limit)
			if err != nil {
				sb.WriteString(fmt.Sprintf("Graph search error: %v\n", err))
			} else if len(triples) == 0 {
				sb.WriteString("No knowledge graph entries found.\n")
			} else {
				sb.WriteString(fmt.Sprintf("### Knowledge Graph (%d results)\n\n", len(triples)))
				for i, t := range triples {
					confidence := ""
					if t.Confidence > 0 {
						confidence = fmt.Sprintf(" (confidence: %.0f%%)", t.Confidence*100)
					}
					sb.WriteString(fmt.Sprintf("%d. %s --[%s]--> %s%s\n",
						i+1, t.Subject, t.Predicate, t.Object, confidence))
				}
			}
		}
	}

	return tools.NewToolResult(sb.String())
}

// --- memory_store ---

// MemoryStoreTool stores new memories (episodes or graph entities/triples).
type MemoryStoreTool struct {
	sp StoreProvider
}

// NewMemoryStoreTool creates a new memory store tool.
func NewMemoryStoreTool(sp StoreProvider) *MemoryStoreTool {
	return &MemoryStoreTool{sp: sp}
}

func (t *MemoryStoreTool) Name() string {
	return "memory_store"
}

func (t *MemoryStoreTool) Description() string {
	return "Store information in memory. Can save episodic memories (conversation experiences) or knowledge graph entities and relationships."
}

func (t *MemoryStoreTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"memory_type": map[string]interface{}{
				"type":        "string",
				"description": "Type of memory to store: 'episodic' or 'graph'",
				"enum":        []string{"episodic", "graph"},
			},
			"content": map[string]interface{}{
				"type":        "string",
				"description": "Content for episodic memory (ignored for graph type)",
			},
			"role": map[string]interface{}{
				"type":        "string",
				"description": "Role for episodic memory: 'user', 'assistant', 'system'",
				"default":     "assistant",
			},
			"tags": map[string]interface{}{
				"type":        "array",
				"description": "Tags for the memory entry",
				"items": map[string]interface{}{
					"type": "string",
				},
			},
			"session_key": map[string]interface{}{
				"type":        "string",
				"description": "Session key for episodic memory (auto-generated if empty)",
			},
			"entity_name": map[string]interface{}{
				"type":        "string",
				"description": "Entity name for graph memory",
			},
			"entity_type": map[string]interface{}{
				"type":        "string",
				"description": "Entity type for graph: 'person', 'place', 'thing', 'concept'",
			},
			"entity_properties": map[string]interface{}{
				"type":        "object",
				"description": "Additional properties for the entity (string key-value pairs)",
			},
			"triple_subject": map[string]interface{}{
				"type":        "string",
				"description": "Subject of a graph triple",
			},
			"triple_predicate": map[string]interface{}{
				"type":        "string",
				"description": "Predicate (relationship type) of a graph triple",
			},
			"triple_object": map[string]interface{}{
				"type":        "string",
				"description": "Object of a graph triple",
			},
			"confidence": map[string]interface{}{
				"type":        "number",
				"description": "Confidence score for the triple (0.0-1.0)",
				"minimum":     0.0,
				"maximum":     1.0,
			},
		},
		"required": []string{"memory_type"},
	}
}

func (t *MemoryStoreTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	memoryType, _ := args["memory_type"].(string)
	if memoryType == "" {
		return tools.ErrorResult("memory_type is required")
	}

	switch memoryType {
	case "episodic":
		return t.storeEpisodic(ctx, args)
	case "graph":
		return t.storeGraph(ctx, args)
	default:
		return tools.ErrorResult(fmt.Sprintf("unknown memory_type: %s (use 'episodic' or 'graph')", memoryType))
	}
}

func (t *MemoryStoreTool) storeEpisodic(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	epStore := t.sp.GetEpisodicStore()
	if epStore == nil {
		return tools.ErrorResult("episodic memory store is not available")
	}

	content, _ := args["content"].(string)
	if content == "" {
		return tools.ErrorResult("content is required for episodic memory")
	}

	role, _ := args["role"].(string)
	if role == "" {
		role = "assistant"
	}

	sessionKey, _ := args["session_key"].(string)
	if sessionKey == "" {
		sessionKey = fmt.Sprintf("manual-%d", time.Now().Unix())
	}

	var tags []string
	if raw, ok := args["tags"].([]interface{}); ok {
		for _, v := range raw {
			if s, ok := v.(string); ok {
				tags = append(tags, s)
			}
		}
	}

	episode := &episodic.Episode{
		SessionKey: sessionKey,
		Role:       role,
		Content:    content,
		Tags:       tags,
	}

	if err := epStore.StoreEpisode(ctx, episode); err != nil {
		return tools.ErrorResult(fmt.Sprintf("failed to store episodic memory: %v", err))
	}

	return tools.NewToolResult(fmt.Sprintf("Episodic memory stored successfully (ID: %s, session: %s)", episode.ID, sessionKey))
}

func (t *MemoryStoreTool) storeGraph(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	gStore := t.sp.GetGraphStore()
	if gStore == nil {
		return tools.ErrorResult("knowledge graph store is not available")
	}

	var results []string

	// Store entity if provided
	entityName, _ := args["entity_name"].(string)
	if entityName != "" {
		entityType, _ := args["entity_type"].(string)
		if entityType == "" {
			entityType = "concept"
		}

		properties := make(map[string]string)
		if raw, ok := args["entity_properties"].(map[string]interface{}); ok {
			for k, v := range raw {
				if s, ok := v.(string); ok {
					properties[k] = s
				}
			}
		}

		entity := &graph.Entity{
			Name:       entityName,
			Type:       entityType,
			Properties: properties,
		}

		if err := gStore.AddEntity(ctx, entity); err != nil {
			return tools.ErrorResult(fmt.Sprintf("failed to store entity: %v", err))
		}
		results = append(results, fmt.Sprintf("Entity stored: %s (%s)", entityName, entityType))
	}

	// Store triple if provided
	subject, _ := args["triple_subject"].(string)
	predicate, _ := args["triple_predicate"].(string)
	object, _ := args["triple_object"].(string)

	if subject != "" && predicate != "" && object != "" {
		confidence := 1.0
		if c, ok := args["confidence"].(float64); ok && c > 0 && c <= 1.0 {
			confidence = c
		}

		triple := &graph.Triple{
			Subject:    subject,
			Predicate:  predicate,
			Object:     object,
			Confidence: confidence,
		}

		if err := gStore.AddTriple(ctx, triple); err != nil {
			return tools.ErrorResult(fmt.Sprintf("failed to store triple: %v", err))
		}
		results = append(results, fmt.Sprintf("Triple stored: %s --[%s]--> %s", subject, predicate, object))
	}

	if len(results) == 0 {
		return tools.ErrorResult("for graph memory, provide entity_name and/or triple_subject+triple_predicate+triple_object")
	}

	return tools.NewToolResult(fmt.Sprintf("Graph memory stored:\n%s", strings.Join(results, "\n")))
}

// --- memory_forget ---

// MemoryForgetTool removes memories from the store.
type MemoryForgetTool struct {
	sp StoreProvider
}

// NewMemoryForgetTool creates a new memory forget tool.
func NewMemoryForgetTool(sp StoreProvider) *MemoryForgetTool {
	return &MemoryForgetTool{sp: sp}
}

func (t *MemoryForgetTool) Name() string {
	return "memory_forget"
}

func (t *MemoryForgetTool) Description() string {
	return "Remove memories. Can delete episodic sessions, cleanup old memories, or remove knowledge graph entities."
}

func (t *MemoryForgetTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"action": map[string]interface{}{
				"type":        "string",
				"description": "Action to perform: 'delete_session', 'cleanup', 'delete_entity'",
				"enum":        []string{"delete_session", "cleanup", "delete_entity"},
			},
			"session_key": map[string]interface{}{
				"type":        "string",
				"description": "Session key to delete (for delete_session action)",
			},
			"older_than_days": map[string]interface{}{
				"type":        "integer",
				"description": "Remove memories older than N days (for cleanup action)",
				"minimum":     1.0,
			},
			"entity_name": map[string]interface{}{
				"type":        "string",
				"description": "Entity name to delete (for delete_entity action)",
			},
		},
		"required": []string{"action"},
	}
}

func (t *MemoryForgetTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	action, _ := args["action"].(string)
	if action == "" {
		return tools.ErrorResult("action is required")
	}

	switch action {
	case "delete_session":
		sessionKey, _ := args["session_key"].(string)
		if sessionKey == "" {
			return tools.ErrorResult("session_key is required for delete_session action")
		}

		epStore := t.sp.GetEpisodicStore()
		if epStore == nil {
			return tools.ErrorResult("episodic memory store is not available")
		}

		if err := epStore.DeleteSession(ctx, sessionKey); err != nil {
			return tools.ErrorResult(fmt.Sprintf("failed to delete session: %v", err))
		}

		return tools.NewToolResult(fmt.Sprintf("Session '%s' deleted successfully", sessionKey))

	case "cleanup":
		olderThanDays := 90
		if d, ok := args["older_than_days"].(float64); ok && int(d) > 0 {
			olderThanDays = int(d)
		}

		epStore := t.sp.GetEpisodicStore()
		if epStore == nil {
			return tools.ErrorResult("episodic memory store is not available")
		}

		removed, err := epStore.Cleanup(ctx, time.Duration(olderThanDays)*24*time.Hour)
		if err != nil {
			return tools.ErrorResult(fmt.Sprintf("cleanup failed: %v", err))
		}

		return tools.NewToolResult(fmt.Sprintf("Cleanup completed: removed %d episodes older than %d days", removed, olderThanDays))

	case "delete_entity":
		entityName, _ := args["entity_name"].(string)
		if entityName == "" {
			return tools.ErrorResult("entity_name is required for delete_entity action")
		}

		gStore := t.sp.GetGraphStore()
		if gStore == nil {
			return tools.ErrorResult("knowledge graph store is not available")
		}

		if err := gStore.DeleteEntity(ctx, entityName); err != nil {
			return tools.ErrorResult(fmt.Sprintf("failed to delete entity: %v", err))
		}

		return tools.NewToolResult(fmt.Sprintf("Entity '%s' and all related triples deleted", entityName))

	default:
		return tools.ErrorResult(fmt.Sprintf("unknown action: %s (use 'delete_session', 'cleanup', or 'delete_entity')", action))
	}
}

// --- memory_list ---

// MemoryListTool lists stored memories and their statistics.
type MemoryListTool struct {
	sp StoreProvider
}

// NewMemoryListTool creates a new memory list tool.
func NewMemoryListTool(sp StoreProvider) *MemoryListTool {
	return &MemoryListTool{sp: sp}
}

func (t *MemoryListTool) Name() string {
	return "memory_list"
}

func (t *MemoryListTool) Description() string {
	return "List stored memories. Shows episodic memory sessions, knowledge graph entities, or related graph entries."
}

func (t *MemoryListTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"list_type": map[string]interface{}{
				"type":        "string",
				"description": "What to list: 'episodes' (recent episodes for a session), 'graph_query' (query triples), 'graph_related' (entities related to a name), or 'status' (summary counts)",
				"default":     "status",
			},
			"session_key": map[string]interface{}{
				"type":        "string",
				"description": "Session key for listing episodes",
			},
			"limit": map[string]interface{}{
				"type":        "integer",
				"description": "Maximum number of results (1-50)",
				"minimum":     1.0,
				"maximum":     50.0,
				"default":     10.0,
			},
			"entity_name": map[string]interface{}{
				"type":        "string",
				"description": "Entity name for graph_related (finds all relationships within depth hops)",
			},
			"depth": map[string]interface{}{
				"type":        "integer",
				"description": "Depth for graph_related search (1-3)",
				"minimum":     1.0,
				"maximum":     3.0,
				"default":     1.0,
			},
			"subject": map[string]interface{}{
				"type":        "string",
				"description": "Filter by subject for graph_query",
			},
			"predicate": map[string]interface{}{
				"type":        "string",
				"description": "Filter by predicate for graph_query",
			},
			"object": map[string]interface{}{
				"type":        "string",
				"description": "Filter by object for graph_query",
			},
		},
	}
}

func (t *MemoryListTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	listType, _ := args["list_type"].(string)
	if listType == "" {
		listType = "status"
	}

	limit := 10
	if l, ok := args["limit"].(float64); ok {
		if int(l) > 0 && int(l) <= 50 {
			limit = int(l)
		}
	}

	switch listType {
	case "status":
		return t.listStatus(ctx)

	case "episodes":
		return t.listEpisodes(ctx, args, limit)

	case "graph_query":
		return t.listGraphQuery(ctx, args)

	case "graph_related":
		return t.listGraphRelated(ctx, args)

	default:
		return tools.ErrorResult(fmt.Sprintf("unknown list_type: %s", listType))
	}
}

func (t *MemoryListTool) listStatus(ctx context.Context) *tools.ToolResult {
	var sb strings.Builder
	sb.WriteString("## Memory Store Status\n\n")

	epStore := t.sp.GetEpisodicStore()
	if epStore != nil {
		if counter, ok := epStore.(interface {
			SessionCount() int
			EpisodeCount() int
		}); ok {
			sb.WriteString(fmt.Sprintf("### Episodic Memory\n"))
			sb.WriteString(fmt.Sprintf("- Sessions: %d\n", counter.SessionCount()))
			sb.WriteString(fmt.Sprintf("- Total episodes: %d\n\n", counter.EpisodeCount()))
		} else {
			sb.WriteString("### Episodic Memory\n- Available\n\n")
		}
	} else {
		sb.WriteString("### Episodic Memory\n- Not available\n\n")
	}

	gStore := t.sp.GetGraphStore()
	if gStore != nil {
		if counter, ok := gStore.(interface {
			EntityCount() int
			TripleCount() int
		}); ok {
			sb.WriteString(fmt.Sprintf("### Knowledge Graph\n"))
			sb.WriteString(fmt.Sprintf("- Entities: %d\n", counter.EntityCount()))
			sb.WriteString(fmt.Sprintf("- Triples: %d\n", counter.TripleCount()))
		} else {
			sb.WriteString("### Knowledge Graph\n- Available\n")
		}
	} else {
		sb.WriteString("### Knowledge Graph\n- Not available\n")
	}

	return tools.NewToolResult(sb.String())
}

func (t *MemoryListTool) listEpisodes(ctx context.Context, args map[string]interface{}, limit int) *tools.ToolResult {
	sessionKey, _ := args["session_key"].(string)
	if sessionKey == "" {
		return tools.ErrorResult("session_key is required for episodes listing")
	}

	epStore := t.sp.GetEpisodicStore()
	if epStore == nil {
		return tools.ErrorResult("episodic memory store is not available")
	}

	episodes, err := epStore.GetRecent(ctx, sessionKey, limit)
	if err != nil {
		return tools.ErrorResult(fmt.Sprintf("failed to list episodes: %v", err))
	}

	if len(episodes) == 0 {
		return tools.NewToolResult(fmt.Sprintf("No episodes found for session: %s", sessionKey))
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("### Recent Episodes for %s (%d results)\n\n", sessionKey, len(episodes)))
	for i, ep := range episodes {
		sb.WriteString(fmt.Sprintf("%d. [%s] %s: %s\n",
			i+1,
			ep.Timestamp.Format("2006-01-02 15:04"),
			ep.Role,
			truncateText(ep.Content, 200)))
		if len(ep.Tags) > 0 {
			sb.WriteString(fmt.Sprintf("   Tags: %s\n", strings.Join(ep.Tags, ", ")))
		}
	}

	return tools.NewToolResult(sb.String())
}

func (t *MemoryListTool) listGraphQuery(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	subject, _ := args["subject"].(string)
	predicate, _ := args["predicate"].(string)
	object, _ := args["object"].(string)

	gStore := t.sp.GetGraphStore()
	if gStore == nil {
		return tools.ErrorResult("knowledge graph store is not available")
	}

	triples, err := gStore.Query(ctx, subject, predicate, object)
	if err != nil {
		return tools.ErrorResult(fmt.Sprintf("query failed: %v", err))
	}

	if len(triples) == 0 {
		return tools.NewToolResult("No matching triples found")
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("### Graph Query Results (%d triples)\n\n", len(triples)))
	for i, t := range triples {
		confidence := ""
		if t.Confidence > 0 && t.Confidence < 1.0 {
			confidence = fmt.Sprintf(" (%.0f%%)", t.Confidence*100)
		}
		sb.WriteString(fmt.Sprintf("%d. %s --[%s]--> %s%s\n",
			i+1, t.Subject, t.Predicate, t.Object, confidence))
		if len(t.Metadata) > 0 {
			metaJSON, _ := json.Marshal(t.Metadata)
			sb.WriteString(fmt.Sprintf("   Metadata: %s\n", string(metaJSON)))
		}
	}

	return tools.NewToolResult(sb.String())
}

func (t *MemoryListTool) listGraphRelated(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	entityName, _ := args["entity_name"].(string)
	if entityName == "" {
		return tools.ErrorResult("entity_name is required for graph_related listing")
	}

	depth := 1
	if d, ok := args["depth"].(float64); ok && int(d) > 0 && int(d) <= 3 {
		depth = int(d)
	}

	gStore := t.sp.GetGraphStore()
	if gStore == nil {
		return tools.ErrorResult("knowledge graph store is not available")
	}

	// Show entity info first
	entity, err := gStore.GetEntity(ctx, entityName)
	if err == nil && entity != nil {
		var sb strings.Builder
		sb.WriteString(fmt.Sprintf("### Entity: %s\n", entity.Name))
		sb.WriteString(fmt.Sprintf("- Type: %s\n", entity.Type))
		if len(entity.Properties) > 0 {
			sb.WriteString("- Properties:\n")
			for k, v := range entity.Properties {
				sb.WriteString(fmt.Sprintf("  - %s: %s\n", k, v))
			}
		}
		sb.WriteString("\n")
	}

	triples, err := gStore.GetRelated(ctx, entityName, depth)
	if err != nil {
		return tools.ErrorResult(fmt.Sprintf("failed to get related entities: %v", err))
	}

	if len(triples) == 0 {
		return tools.NewToolResult(fmt.Sprintf("No relationships found for: %s", entityName))
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("### Related to %s (depth=%d, %d triples)\n\n", entityName, depth, len(triples)))
	for i, t := range triples {
		sb.WriteString(fmt.Sprintf("%d. %s --[%s]--> %s\n",
			i+1, t.Subject, t.Predicate, t.Object))
	}

	return tools.NewToolResult(sb.String())
}

// truncateText truncates text to maxLen characters, adding "..." if truncated.
func truncateText(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	if maxLen > 3 {
		return s[:maxLen-3] + "..."
	}
	return s[:maxLen]
}
