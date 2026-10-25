//! 12 题固定任务集（外循环 P4 防倒退门禁；总控计划 §9.1「真实模型任务集」）。
//!
//! 构成：Bug 修复 4 / 文档维护 4 / 测试补充 4——每题是一个自包含迷你仓
//! （seed 文件随 [`TASKS`] 内联声明，run 时物化到隔离临时工作区），配
//! **冻结验收**（statement 内写明验收标准，宿主侧检查器逐一核验）。
//!
//! 纪律（§9.1）：检查器全部在宿主侧执行（评估器不进任务工作区，agent
//! 读不到预期补丁）；所有任务在未动过的 seed 上必须判 FAIL（防误报，
//! §9.2「误报通过 = 0」——由 tests.rs 全集钉死）。

/// 任务三类（§9.1 固定配比 4/4/4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    BugFix,
    Docs,
    Tests,
}

impl Category {
    pub fn slug(&self) -> &'static str {
        match self {
            Category::BugFix => "bugfix",
            Category::Docs => "docs",
            Category::Tests => "tests",
        }
    }
}

/// 宿主侧检查步骤（全部通过 = 任务通过）。
#[derive(Debug, Clone)]
pub enum CheckStep {
    /// 文件包含任一子串（any_of 命中一即过）。
    Contains {
        path: &'static str,
        any_of: &'static [&'static str],
    },
    /// 文件不含任一子串（任一命中即败——防「原样保留旧物」蒙混）。
    NotContains {
        path: &'static str,
        any_of: &'static [&'static str],
    },
    /// Python 行为断言：代码在隔离临时目录执行（cwd=任务工作区，
    /// `sys.path` 注入工作区根），exit 0 = 过。评估器脚本不落工作区。
    ExecPython { code: &'static str },
}

/// 单题定义。
#[derive(Debug, Clone)]
pub struct TaskDef {
    pub id: &'static str,
    pub category: Category,
    /// 任务陈述（发給 agent 的完整 prompt；验收标准冻结在内）。
    pub statement: &'static str,
    /// seed 文件（相对工作区根）。物化时逐字节写出。
    pub seed: &'static [(&'static str, &'static str)],
    pub checkers: &'static [CheckStep],
}

const DISCOVERY_CODE: &str = r#"
import unittest, os, sys
sys.path.insert(0, os.getcwd())
suite = unittest.defaultTestLoader.discover(os.getcwd(), pattern="test_*.py")
res = unittest.TextTestRunner(verbosity=0).run(suite)
sys.exit(0 if res.wasSuccessful() else 1)
"#;

pub static TASKS: &[TaskDef] = &[
    // ------------------------------------------------------------------
    // Bug 修复 ×4
    // ------------------------------------------------------------------
    TaskDef {
        id: "bugfix-sum-off-by-one",
        category: Category::BugFix,
        statement: "utils.py 的 sum_to(n) 存在 off-by-one：sum_to(5) 返回 10 而不是 15（range(1, n) 少算了 n 本身）。\
请修复使语义与 docstring 一致。\
验收（冻结）：sum_to(1) == 1，sum_to(5) == 15，sum_to(0) == 0；不改变函数名与签名。",
        seed: &[
            (
                "utils.py",
                "\"\"\"小型数值工具。\"\"\"\n\n\ndef sum_to(n):\n    \"\"\"返回 1..n（含 n）的整数和。\"\"\"\n    total = 0\n    for i in range(1, n):\n        total += i\n    return total\n",
            ),
            (
                "main.py",
                "from utils import sum_to\n\nif __name__ == \"__main__\":\n    print(sum_to(5))\n",
            ),
        ],
        checkers: &[CheckStep::ExecPython {
            code: "from utils import sum_to\nassert sum_to(1) == 1, sum_to(1)\nassert sum_to(5) == 15, sum_to(5)\nassert sum_to(0) == 0, sum_to(0)\n",
        }],
    },
    TaskDef {
        id: "bugfix-mutable-default",
        category: Category::BugFix,
        statement: "history.py 的 append_to 使用了可变默认参数（items=[]）：多次调用共享同一个列表，调用方互相污染。\
请修复使每次调用在未显式传 items 时都从空列表开始，且保持函数名、签名兼容（显式传入 items 的调用行为不变）。\
验收（冻结）：append_to(1) 返回 [1]；随后 append_to(2) 返回 [2] 而不是 [1, 2]；\
append_to(9, [0]) 返回 [0, 9]。",
        seed: &[(
            "history.py",
            "\"\"\"调用历史记录工具。\"\"\"\n\n\ndef append_to(item, items=[]):\n    items.append(item)\n    return items\n",
        )],
        checkers: &[CheckStep::ExecPython {
            code: "from history import append_to\nassert append_to(1) == [1]\nassert append_to(2) == [2], \"默认列表被共享\"\nassert append_to(9, [0]) == [0, 9]\n",
        }],
    },
    TaskDef {
        id: "bugfix-integer-division",
        category: Category::BugFix,
        statement: "stats.py 的 average 使用整除（//），对浮点输入返回截断值：average([1, 2]) 返回 1 而不是 1.5。\
请修复使平均数保留浮点精度。\
验收（冻结）：average([1, 2]) == 1.5；average([2, 4]) == 3.0；average([5]) == 5；空列表抛 ValueError。\
不改变函数名与签名。",
        seed: &[(
            "stats.py",
            "\"\"\"简单统计。\"\"\"\n\n\ndef average(xs):\n    \"\"\"返回数值列表的平均数。\"\"\"\n    if not xs:\n        raise ValueError(\"empty\")\n    return sum(xs) // len(xs)\n",
        )],
        checkers: &[CheckStep::ExecPython {
            code: "from stats import average\nassert average([1, 2]) == 1.5, average([1, 2])\nassert average([2, 4]) == 3.0\nassert average([5]) == 5\ntry:\n    average([])\nexcept ValueError:\n    pass\nelse:\n    raise AssertionError(\"空列表未抛 ValueError\")\n",
        }],
    },
    TaskDef {
        id: "bugfix-portable-path",
        category: Category::BugFix,
        statement: "files.py 的 config_path 用字符串拼接构造子路径（\"config/\" + name），在 Windows 上依赖正斜杠被容忍，\
且 name 以分隔符开头时产生错误路径。请改用 os.path.join 构造，使其与平台原生路径语义一致。\
验收（冻结）：config_path(\"a.toml\") == os.path.join(\"config\", \"a.toml\")；\
config_path(\"sub\\\\b.toml\")（Windows）或 config_path(\"sub/b.toml\")（POSIX）== os.path.join(\"config\", 对应相对路径)；\
config_path 对以分隔符开头的 name（\"/x.toml\" 或 \"\\\\x.toml\"）返回值恒等于 os.path.join(\"config\", 该 name)（os.path.join 绝对段重置语义）。\
不改变函数名与签名。",
        seed: &[(
            "files.py",
            "\"\"\"配置文件路径工具。\"\"\"\n\n\ndef config_path(name):\n    \"\"\"返回 name 对应的 config 子路径。\"\"\"\n    return \"config/\" + name\n",
        )],
        checkers: &[CheckStep::ExecPython {
            // 尾带「分隔符开头」断言（§statement 冻结验收原话）：os.path.join 对绝对
            // 段重置语义在 POSIX/Windows 一致，拼接臂在两个平台都判败——只测嵌入
            // 分隔符的话 POSIX 上 seed 恰好等于 join 结果，防误报在 Linux 失守
            //（2026-10-25 ET Linux 全量实证）。
            code: "import os\nfrom files import config_path\nsep = \"\\\\\" if os.name == \"nt\" else \"/\"\nassert config_path(\"a.toml\") == os.path.join(\"config\", \"a.toml\")\nassert config_path(\"sub\" + sep + \"b.toml\") == os.path.join(\"config\", \"sub\" + sep + \"b.toml\")\nassert config_path(sep + \"x.toml\") == os.path.join(\"config\", sep + \"x.toml\")\n",
        }],
    },
    // ------------------------------------------------------------------
    // 文档维护 ×4
    // ------------------------------------------------------------------
    TaskDef {
        id: "docs-readme-usage",
        category: Category::Docs,
        statement: "kvstore 是一个小型键值 CLI（见 README.md 与 kvstore.py），但 README 缺少使用说明。\
请在 README.md 中新增 `## Usage` 小节（二级标题，恰好写作 ## Usage），介绍命令行用法，\
并说明 `--name` 标志的作用（指定键值存储文件名，默认 kv.db）。\
验收（冻结）：README.md 存在 `## Usage` 二级标题；该节提及 `--name` 与默认文件名 kv.db。",
        seed: &[
            (
                "README.md",
                "# kvstore\n\n极简键值存储命令行工具：把键值对持久化到一个本地文件。\n\n## Features\n\n- `put key value` 写入\n- `get key` 读取\n- `--name` 自定义存储文件\n",
            ),
            (
                "kvstore.py",
                "\"\"\"kvstore 命令行入口。\"\"\"\n\n\ndef main():\n    pass\n",
            ),
        ],
        checkers: &[
            CheckStep::Contains {
                path: "README.md",
                any_of: &["## Usage"],
            },
            CheckStep::Contains {
                path: "README.md",
                any_of: &["--name"],
            },
            CheckStep::Contains {
                path: "README.md",
                any_of: &["kv.db"],
            },
        ],
    },
    TaskDef {
        id: "docs-changelog-020",
        category: Category::Docs,
        statement: "项目刚发布 0.2.0，修复了统计器「重复计时」的问题（同一区间被计了两次）。\
请在 CHANGELOG.md 顶部按既有格式新增 `## [0.2.0]` 段落，记录该修复。\
验收（冻结）：CHANGELOG.md 存在 `## [0.2.0]` 段落；该段落提及「重复计时」；既有 [0.1.0] 内容原样保留。",
        seed: &[(
            "CHANGELOG.md",
            "# Changelog\n\n## [0.1.0] - 2026-09-01\n\n- 首个可用版本：基础计时与汇总。\n",
        )],
        checkers: &[
            CheckStep::Contains {
                path: "CHANGELOG.md",
                any_of: &["## [0.2.0]"],
            },
            CheckStep::Contains {
                path: "CHANGELOG.md",
                any_of: &["重复计时"],
            },
            CheckStep::Contains {
                path: "CHANGELOG.md",
                any_of: &["## [0.1.0]"],
            },
        ],
    },
    TaskDef {
        id: "docs-docstring-area",
        category: Category::Docs,
        statement: "shapes.py 的 area(r) 缺少 docstring。请为它补一个 docstring，说明参数 r 的含义（圆的半径）与返回值（圆面积）。\
验收（冻结）：area 函数带三引号 docstring；docstring 说明 r 是半径（出现「半径」或 radius）。",
        seed: &[(
            "shapes.py",
            "\"\"\"几何计算。\"\"\"\n\n\ndef area(r):\n    return 3.14159 * r * r\n",
        )],
        checkers: &[
            CheckStep::Contains {
                path: "shapes.py",
                any_of: &["\"\"\""],
            },
            CheckStep::Contains {
                path: "shapes.py",
                any_of: &["半径", "radius"],
            },
        ],
    },
    TaskDef {
        id: "docs-api-ref-parse-kv",
        category: Category::Docs,
        statement: "docs/api.md 只记载了 parse_line，缺少同模块 kv.py 中 parse_kv 的说明。\
请在 docs/api.md 增补 parse_kv 小节，说明其 delimiter 参数默认值是英文逗号 `,`。\
验收（冻结）：docs/api.md 提及 parse_kv 与 delimiter，并写明默认值为逗号（`,`）。",
        seed: &[
            (
                "kv.py",
                "def parse_line(line, delimiter=\",\"):\n    return line.split(delimiter)\n\n\ndef parse_kv(line, delimiter=\",\"):\n    \"\"\"把 'k=v' 行解析成 (k, v)，按 delimiter 分隔。\"\"\"\n    k, _, v = line.partition(delimiter)\n    return k.strip(), v.strip()\n",
            ),
            (
                "docs/api.md",
                "# API 参考\n\n## parse_line(line, delimiter=\",\")\n\n按分隔符切分一行，返回列表。\n",
            ),
        ],
        checkers: &[
            CheckStep::Contains {
                path: "docs/api.md",
                any_of: &["parse_kv"],
            },
            CheckStep::Contains {
                path: "docs/api.md",
                any_of: &["delimiter"],
            },
            CheckStep::Contains {
                path: "docs/api.md",
                any_of: &["`,`", "`,`）", "\",\""],
            },
        ],
    },
    // ------------------------------------------------------------------
    // 测试补充 ×4
    // ------------------------------------------------------------------
    TaskDef {
        id: "tests-add-slugify",
        category: Category::Tests,
        statement: "string_utils.py 提供 slugify 但 test_string_utils.py 没有覆盖它。\
请按既有 unittest 风格为 slugify 补测试：至少一个函数名以 test_slugify 开头的测试用例，\
断言 slugify(\"Hello World\") == \"hello-world\"。\
验收（冻结）：测试文件存在 test_slugify* 用例；全量 unittest 通过（python -m unittest discover）。",
        seed: &[
            (
                "string_utils.py",
                "def normalize(s):\n    return s.strip().lower()\n\n\ndef slugify(s):\n    return normalize(s).replace(\" \", \"-\")\n",
            ),
            (
                "test_string_utils.py",
                "import unittest\n\nfrom string_utils import normalize\n\n\nclass TestNormalize(unittest.TestCase):\n    def test_normalize_strips_and_lowers(self):\n        self.assertEqual(normalize(\"  Hi  \"), \"hi\")\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
            ),
        ],
        checkers: &[
            CheckStep::Contains {
                path: "test_string_utils.py",
                any_of: &["def test_slugify"],
            },
            CheckStep::ExecPython {
                code: DISCOVERY_CODE,
            },
        ],
    },
    TaskDef {
        id: "tests-fix-broken-assert",
        category: Category::Tests,
        statement: "test_math_ops.py 中 test_add 的断言写错了（assertEqual(add(2, 2), 5)），与 math_ops.add 的真实契约不符。\
请修正该断言使测试反映真实契约（2 + 2 == 4），不要改动被测代码。\
验收（冻结）：test_add 断言 add(2, 2) == 4；全量 unittest 通过。",
        seed: &[
            ("math_ops.py", "def add(a, b):\n    return a + b\n"),
            (
                "test_math_ops.py",
                "import unittest\n\nfrom math_ops import add\n\n\nclass TestAdd(unittest.TestCase):\n    def test_add(self):\n        self.assertEqual(add(2, 2), 5)\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
            ),
        ],
        checkers: &[
            CheckStep::Contains {
                path: "test_math_ops.py",
                any_of: &["add(2, 2), 4", "add(2,2), 4"],
            },
            CheckStep::ExecPython {
                code: DISCOVERY_CODE,
            },
        ],
    },
    TaskDef {
        id: "tests-edge-clamp-empty",
        category: Category::Tests,
        statement: "clamp.py 的 clamp(xs, lo, hi) 对空列表返回空列表，但 test_clamp.py 没有覆盖空输入边界。\
请新增一个名为 test_clamp_empty 的测试用例，断言 clamp([], 0, 1) == []。\
验收（冻结）：测试文件存在 test_clamp_empty 用例；全量 unittest 通过。",
        seed: &[
            (
                "clamp.py",
                "def clamp(xs, lo, hi):\n    \"\"\"把每个元素夹到 [lo, hi]。空列表返回空列表。\"\"\"\n    return [min(max(x, lo), hi) for x in xs]\n",
            ),
            (
                "test_clamp.py",
                "import unittest\n\nfrom clamp import clamp\n\n\nclass TestClamp(unittest.TestCase):\n    def test_clamp_basic(self):\n        self.assertEqual(clamp([1, 5, 10], 2, 8), [2, 5, 8])\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
            ),
        ],
        checkers: &[
            CheckStep::Contains {
                path: "test_clamp.py",
                any_of: &["def test_clamp_empty"],
            },
            CheckStep::ExecPython {
                code: DISCOVERY_CODE,
            },
        ],
    },
    TaskDef {
        id: "tests-consolidate-subtest",
        category: Category::Tests,
        statement: "test_format.py 里 test_format_a/b/c 三个用例几乎完全重复。\
请合并成单个参数化用例：新用例命名为 test_format_cases，用 self.subTest 覆盖原有三组输入输出，\
并删除原来的三个重复用例。不要改动被测代码。\
验收（冻结）：测试文件存在 test_format_cases 且使用 subTest；test_format_a/b/c 不再存在；全量 unittest 通过。",
        seed: &[
            ("format.py", "def fmt(n):\n    return format(n, \",d\")\n"),
            (
                "test_format.py",
                "import unittest\n\nfrom format import fmt\n\n\nclass TestFmt(unittest.TestCase):\n    def test_format_a(self):\n        self.assertEqual(fmt(1000), \"1,000\")\n\n    def test_format_b(self):\n        self.assertEqual(fmt(1000000), \"1,000,000\")\n\n    def test_format_c(self):\n        self.assertEqual(fmt(0), \"0\")\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
            ),
        ],
        checkers: &[
            CheckStep::Contains {
                path: "test_format.py",
                any_of: &["def test_format_cases"],
            },
            CheckStep::Contains {
                path: "test_format.py",
                any_of: &["subTest"],
            },
            CheckStep::NotContains {
                path: "test_format.py",
                any_of: &[
                    "def test_format_a",
                    "def test_format_b",
                    "def test_format_c",
                ],
            },
            CheckStep::ExecPython {
                code: DISCOVERY_CODE,
            },
        ],
    },
];

/// 任务集完整性校验（12 题 / 4+4+4 / id 唯一 / statement 带冻结验收 /
/// seed 与 checkers 非空）。返回违规明细；空 = 合法。
pub fn validate() -> Vec<String> {
    let mut problems = Vec::new();
    if TASKS.len() != 12 {
        problems.push(format!("题数 {} ≠ 12", TASKS.len()));
    }
    let mut ids: Vec<&str> = Vec::new();
    let mut per_cat = [0usize; 3];
    for t in TASKS {
        if t.id.is_empty() {
            problems.push("存在空 id".to_string());
        }
        if ids.contains(&t.id) {
            problems.push(format!("id 重复: {}", t.id));
        }
        ids.push(t.id);
        per_cat[match t.category {
            Category::BugFix => 0,
            Category::Docs => 1,
            Category::Tests => 2,
        }] += 1;
        if !t.statement.contains("验收（冻结）") {
            problems.push(format!("{}: statement 缺冻结验收段", t.id));
        }
        if t.seed.is_empty() {
            problems.push(format!("{}: seed 为空", t.id));
        }
        if t.checkers.is_empty() {
            problems.push(format!("{}: checkers 为空", t.id));
        }
    }
    if per_cat != [4, 4, 4] {
        problems.push(format!("类别配比 {per_cat:?} ≠ [4, 4, 4]"));
    }
    problems
}
