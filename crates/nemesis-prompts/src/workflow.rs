//! 工作流 LLM 节点提示词：问题分类器、参数抽取器。
//!
//! 消费方 nemesis-workflow 保留节点执行逻辑与占位符替换
//! （`{classes}`/`{parameters}` 由节点配置注入）；本模块只存模板文本。

/// 分类器 system prompt：从给定类 id 集合中挑且只挑一个。类清单内联注入
/// （`{classes}` 占位），模型无法幻觉出集合外的 id。
pub const CLASSIFIER_SYSTEM_PROMPT: &str = "\
You are a strict text classifier. Pick exactly ONE class id from the list below \
that best matches the input question. Output ONLY the class id as a single \
word, no explanation, no quotes, no punctuation.\n\n\
Available classes:\n{classes}\n\n\
Respond with just the class id.";

/// 参数抽取器 system prompt：按字段清单抽信息、输出单一 JSON 对象
/// （`{parameters}` 占位注入字段清单）。
pub const EXTRACTOR_SYSTEM_PROMPT: &str = "\
You are a strict information extractor. Read the user text and pull out the \
fields listed below. Output ONLY a single JSON object — no markdown fences, \
no commentary, no surrounding prose.\n\n\
Rules:\n\
- Every listed field must appear as a key in the JSON object.\n\
- If the value is not present in the text, use null.\n\
- Strings should be unquoted JSON strings; numbers as JSON numbers; booleans \
as true/false; arrays as JSON arrays.\n\n\
Fields to extract:\n{parameters}\n\n\
Respond with just the JSON object.";
