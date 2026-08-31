package models

import (
	"fmt"
	"strings"
)

// ParseAliases 解析 TESTAI_ALIASES 别名表（W1.5：TestAIServer 别名模拟）。
//
// 格式：逗号分隔的 `别名=目标` 对，例如：
//
//	TESTAI_ALIASES=gpt-4o-mini=testai-1.1,gpt-4.1=testai-2.0
//
// 目的：让价目表（model_prices）内已知的真实模型名映射到硬编码测试模型，
// 使 `total_cost_usd ≠ 0` 的计价管线断言不依赖真实模型环境。硬编码测试
// 模型本身的行为零改动——别名只在 registry.Get 未命中时参与解析。
//
// 解析规则：
//   - 段与 `=` 两侧空白忽略；
//   - 缺 `=`、别名为空、目标为空的段报错并跳过（不致命，不影响其余别名）；
//   - 别名链（别名指向别名）不支持——目标必须是已注册的 testai-* 真名，
//     Get 只解析一层。
//
// 返回解析成功的别名表与逐条错误（调用方决定如何呈现）。
func ParseAliases(spec string) (map[string]string, []error) {
	aliases := make(map[string]string)
	var errs []error

	for _, seg := range strings.Split(spec, ",") {
		seg = strings.TrimSpace(seg)
		if seg == "" {
			continue
		}
		eq := strings.Index(seg, "=")
		if eq < 0 {
			errs = append(errs, fmt.Errorf("别名段 %q 缺少 '='，忽略", seg))
			continue
		}
		alias := strings.TrimSpace(seg[:eq])
		target := strings.TrimSpace(seg[eq+1:])
		if alias == "" || target == "" {
			errs = append(errs, fmt.Errorf("别名段 %q 别名或目标为空，忽略", seg))
			continue
		}
		aliases[alias] = target
	}
	return aliases, errs
}

// SetAlias 注册单个别名。校验：
//   - 别名不得与已注册模型同名（硬编码优先，零行为改动）；
//   - 目标必须已注册（别名指向别名不支持）。
func (r *ModelRegistry) SetAlias(alias, target string) error {
	if alias == "" || target == "" {
		return fmt.Errorf("别名/目标不能为空")
	}
	if _, exists := r.models[alias]; exists {
		return fmt.Errorf("别名 %q 与已注册模型同名，忽略（硬编码优先）", alias)
	}
	if _, exists := r.models[target]; !exists {
		return fmt.Errorf("别名 %q 的目标模型 %q 未注册，忽略", alias, target)
	}
	r.aliases[alias] = target
	return nil
}

// ApplyAliases 批量装入别名表，返回逐条错误（一条失败不影响其余）。
func (r *ModelRegistry) ApplyAliases(aliases map[string]string) []error {
	var errs []error
	for alias, target := range aliases {
		if err := r.SetAlias(alias, target); err != nil {
			errs = append(errs, err)
		}
	}
	return errs
}

// Aliases 返回当前别名表的拷贝（只读视图用）。
func (r *ModelRegistry) Aliases() map[string]string {
	out := make(map[string]string, len(r.aliases))
	for k, v := range r.aliases {
		out[k] = v
	}
	return out
}
