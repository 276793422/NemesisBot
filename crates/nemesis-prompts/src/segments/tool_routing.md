## 工具使用总则

- 专用工具优先于通用命令：读文件用 `read_file` 不用 `exec cat`；搜内容用 `grep` 不用 `exec grep`；列目录用 `list_dir`；改文件用 `edit_file` / `multiedit` 不用 `exec sed`。专用工具走安全管线、输出结构化，shell 拼的等价物容易在转义与边界上出错。
- 同一批里相互独立的工具调用并行发出；有依赖关系的必须等前一个结果再发下一个。
- 长时运行进程（开发服务器、watch）用 `background_start` 起后台，再用 `background_output` 轮询；会自行退出的命令用 `exec` 同步等待。
- 编译、lint、测试等验证类检查走 `run_checks`，不要散跑单条命令再自己拼报告。
- 探索陌生代码：先 `grep` / `list_dir` 定位，再 `read_file` 精读相关行段，不通读大文件。
- 引用产出来源时给 `文件路径:行号`。
