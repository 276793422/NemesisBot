package models

import (
	"fmt"
	"time"
)

// TestAIVision - 多模态视觉测试模型
// 功能：检测请求是否包含图像内容部分（content 数组中的 image_url part）
//   - 含图：回复 VISION_OK:<图片数>（确定性断言，E2E 用）
//   - 不含图：回复 NO_IMAGE
//
// 用途：NemesisBot 多模态管线的确定性 E2E——不依赖真实视觉模型即可断言
// 「图像 content part 成功穿过 provider 序列化到达 LLM 请求体」。
type TestAIVision struct{}

func NewTestAIVision() *TestAIVision {
	return &TestAIVision{}
}

func (m *TestAIVision) Name() string {
	return "testai-vision-1.0"
}

func (m *TestAIVision) Process(messages []Message) string {
	count := 0
	for _, msg := range messages {
		for _, p := range msg.Parts {
			if p.Type == "image_url" && p.ImageURL != nil && p.ImageURL.URL != "" {
				count++
			}
		}
	}
	if count > 0 {
		return fmt.Sprintf("VISION_OK:%d", count)
	}
	return "NO_IMAGE"
}

func (m *TestAIVision) Delay() time.Duration {
	return 0
}
