package handlers

import (
	"net/http"
	"time"

	"github.com/gin-gonic/gin"
)

// ImageGenerations OpenAI images API 假端点（职能框架 M7）：
// POST /v1/images/generations → 固定返回 1×1 PNG（b64_json）。
//
// 供 nemesis-providers images lane（generate_image）与 UAT U5（UI 子单产出
// PNG → 资产取回 → vision 评审）联验：请求 {model,prompt,n,response_format,
// size?}，响应 {created,data:[{b64_json}]}。字节级确定（零随机），任何
// prompt 都返回同一张图——UAT 断言只验「落盘文件是合法 PNG + sha256 与
// 资产清单一致」，不验像素语义。
//
// prompt 含 <IMG_BAD> → 500（错误臂联验：契约降级诚实报错路径）。
func (h *Handler) ImageGenerations(c *gin.Context) {
	var req struct {
		Model          string `json:"model"`
		Prompt         string `json:"prompt"`
		N              int    `json:"n"`
		ResponseFormat string `json:"response_format"`
		Size           string `json:"size"`
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{
			"error": gin.H{
				"message": "Invalid request format",
				"type":    "invalid_request_error",
				"code":    "invalid_json",
			},
		})
		return
	}

	if req.Prompt == "" {
		c.JSON(http.StatusBadRequest, gin.H{
			"error": gin.H{
				"message": "prompt is required",
				"type":    "invalid_request_error",
				"code":    "missing_prompt",
			},
		})
		return
	}

	// 错误臂：prompt 带 <IMG_BAD> → 上游故障形态。
	for _, bad := range []string{"<IMG_BAD>"} {
		if len(req.Prompt) >= len(bad) && contains(req.Prompt, bad) {
			c.JSON(http.StatusInternalServerError, gin.H{
				"error": gin.H{
					"message": "simulated upstream image failure",
					"type":    "server_error",
					"code":    "img_bad_marker",
				},
			})
			return
		}
	}

	// 1×1 PNG（与 nemesisbot board_review_assets 测试 / UAT 断言同源字节）。
	const tinyPNGB64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVQI12P4z8AAAAMBAQAY3Y2wAAAAAElFTkSuQmCC"

	c.JSON(http.StatusOK, gin.H{
		"created": time.Now().Unix(),
		"data": []gin.H{
			{"b64_json": tinyPNGB64},
		},
	})
}

// contains 简化子串判断（避免为单一用途引 strings 包别名冲突）。
func contains(s, sub string) bool {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}
