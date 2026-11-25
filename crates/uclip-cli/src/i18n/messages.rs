pub type Message = (&'static str, &'static str);
pub const ABOUT: Message = (
    "UClip clipboard history and synchronization",
    "UClip 剪贴板历史和同步",
);
pub const LANGUAGE_OVERRIDE: Message = (
    "Set UCLIP_LANG to en, zh-CN, or system to choose the help and diagnostics language.",
    "将 UCLIP_LANG 设为 en、zh-CN 或 system，可选择帮助和诊断的语言。",
);
pub const REGEX: Message = (
    "Interpret the query as a regular expression.",
    "将搜索词视为正则表达式。",
);
pub const SYNC_REMOTE_LONG: Message = (
    concat!(
        "Read a synchronization configuration in JSON from stdin.\n\n",
        "WebDAV fields: kind (webdav), url (HTTPS), username, password.\n",
        "SFTP fields: kind (sftp), host, port, directory (absolute), username, password, hostKey (SHA256 fingerprint).",
    ),
    concat!(
        "从标准输入读取 JSON 格式的同步配置。\n\n",
        "WebDAV 字段：kind（webdav）、url（HTTPS）、username、password。\n",
        "SFTP 字段：kind（sftp）、host、port、directory（绝对路径）、username、password、hostKey（SHA256 指纹）。",
    ),
);
pub const CLIPBOARD_USAGE_FAILED: Message = (
    "clipboard written; usage could not be saved",
    "已写入剪贴板，但无法保存使用记录",
);
pub const RECORD_SAVED: Message = ("record saved", "记录已保存");
pub const SYNC_SETUP_REQUIRED: Message = (
    "configure a synchronization group first with 'uclip sync create' or 'uclip sync join'",
    "请先运行 'uclip sync create' 或 'uclip sync join' 配置同步组",
);
pub const SYNC_STARTED: Message = ("synchronization service started", "同步服务已启动");
pub const DATA_DIR: Message = (
    "Explicit UClip library directory (also supported via UCLIP_DATA_DIR).",
    "显式指定 UClip 数据库目录。也可使用 UCLIP_DATA_DIR。",
);
pub const DEV: Message = (
    "Use the library of npm run tauri:dev.",
    "使用 npm run tauri:dev 的数据库。",
);
pub const PORTABLE: Message = (
    "Use uclip-data beside this executable.",
    "使用可执行文件旁的 uclip-data。",
);
pub const GET: Message = (
    "Read a record by UUID, or the newest record in history.",
    "按 UUID 读取记录，或读取历史中的最新记录。",
);
pub const LIST: Message = (
    "List active history in desktop order.",
    "按桌面版顺序列出活动历史。",
);
pub const SEARCH: Message = ("Search text in active history.", "在活动历史中搜索文本。");
pub const COPY: Message = ("Write to the system clipboard.", "写入系统剪贴板。");
pub const SAVE: Message = (
    "Save text, an image, or explicit file references.",
    "保存文本、图片或显式指定的文件引用。",
);
pub const IMPORT: Message = (
    "Preview or import a history archive into a standalone library.",
    "预演或向未加入同步组的库导入历史归档。",
);
pub const EXPORT: Message = (
    "Export history, collections, and Trash as a JSON Lines archive.",
    "将历史、分类和回收站导出为 JSON Lines 归档。",
);
pub const ARCHIVE_INPUT: Message = (
    "Read the archive from a file. Defaults to stdin.",
    "从文件读取归档。默认从标准输入读取。",
);
pub const ARCHIVE_OUTPUT: Message = (
    "Write to a new file. Defaults to stdout.",
    "写入新文件。默认写入标准输出。",
);
pub const IMPORT_APPLY: Message = (
    "Commit the import after backing up the library. Defaults to preview.",
    "备份库后提交导入。默认为预演。",
);
pub const IMPORT_SKIP_INVALID: Message = (
    "Report and skip records whose payload cannot be imported.",
    "报告并跳过无法导入内容的记录。",
);
pub const ARCHIVE_SKIPPED: Message = ("skipped", "已跳过");
pub const COMPLETION: Message = ("Generate shell completions.", "生成 Shell 补全脚本。");
pub const SYNC: Message = ("Configure a synchronization group.", "配置同步组。");
pub const SERVE: Message = (
    "Run encrypted synchronization without a desktop.",
    "无需桌面环境即可运行加密同步。",
);
pub const RECORD_ID: Message = (
    "Record UUID or latest. Defaults to the first record in the selected scope.",
    "记录 UUID 或 latest。默认使用当前范围内的第一条记录。",
);
pub const POSITION: Message = (
    "One-based position in desktop list order.",
    "桌面列表中的位置，从 1 开始。",
);
pub const INDEX: Message = (
    "Zero-based position in desktop list order.",
    "桌面列表中的索引，从 0 开始。",
);
pub const SELECT_COLLECTION: Message = (
    "Select an active collection by name or UUID.",
    "按名称或 UUID 选择一个活动分类。",
);
pub const GET_COPY: Message = (
    "Also write the record to the system clipboard.",
    "同时将记录写入系统剪贴板。",
);
pub const RAW: Message = (
    "Output original bytes without terminal escaping or an added newline.",
    "输出原始字节，保留控制字符且不追加换行。",
);
pub const LIMIT: Message = ("Maximum records to return.", "最多返回的记录数。");
pub const JSON: Message = ("Output one JSON history page.", "以 JSON 输出一页历史。");
pub const FULL: Message = (
    "Output JSON Lines with full content. Images are base64 PNG.",
    "逐行输出包含完整内容的 JSON。图片采用 Base64 编码的 PNG。",
);
pub const TEXT: Message = (
    "Literal text. With no source, read UTF-8 text from stdin.",
    "直接提供文本。未指定来源时，从标准输入读取 UTF-8 文本。",
);
pub const FILE: Message = (
    "Read UTF-8 text from a file, or - for stdin.",
    "从文件读取 UTF-8 文本。使用 - 从标准输入读取。",
);
pub const IMAGE: Message = (
    "Read an image from a file, or - for stdin.",
    "从文件读取图片。使用 - 从标准输入读取。",
);
pub const REFERENCE: Message = (
    "Reference a file or directory without reading its contents. Repeat to add more paths.",
    "引用文件或目录，不读取其内容。重复此选项可添加多个路径。",
);
pub const SAVE_COLLECTION: Message = (
    "Save into an existing collection by name or UUID.",
    "按名称或 UUID 指定要保存到的现有分类。",
);
pub const SAVE_COPY: Message = ("Also copy the saved content.", "同时复制已保存的内容。");
pub const COPY_ID: Message = ("Copy an existing record by UUID.", "按 UUID 复制现有记录。");
pub const COPY_SAVE: Message = (
    "Save the input to history as well.",
    "同时将输入保存到历史。",
);
pub const SYNC_CREATE: Message = (
    "Create a new group with LAN discovery enabled.",
    "创建一个启用局域网发现的新同步组。",
);
pub const SYNC_JOIN: Message = (
    "Join an existing group using a pairing code read from stdin.",
    "从标准输入读取配对码并加入现有同步组。",
);
pub const SYNC_FROM_NOW: Message = (
    "Keep existing clipboard records on this device and sync new records.",
    "将已有剪贴板记录保留在本机，只同步新记录。",
);
pub const SYNC_CODE: Message = (
    "Print the pairing code to stdout for deliberate sharing.",
    "将配对码写到标准输出，以便手动分享。",
);
pub const SYNC_STATUS: Message = (
    "Print the saved configuration without credentials.",
    "输出已保存的配置，省略凭据。",
);
pub const SYNC_LAN: Message = (
    "Enable or disable LAN discovery and synchronization.",
    "开启或关闭局域网发现和同步。",
);
pub const SYNC_REMOTE: Message = (
    "Read a WebDAV or SFTP configuration in JSON from stdin.",
    "从标准输入读取 JSON 格式的 WebDAV 或 SFTP 配置。",
);
pub const SYNC_CLEAR_REMOTE: Message = (
    "Disable remote storage synchronization.",
    "关闭远端存储同步。",
);

pub const QUERY: Message = ("Text to search for.", "要搜索的文本。");
pub const SHELL: Message = (
    "Shell to generate completions for.",
    "要生成补全脚本的 Shell。",
);
pub const LAN_ENABLED: Message = (
    "Whether LAN synchronization is enabled.",
    "是否开启局域网同步。",
);

pub const CLAP_HELP_TEXT: &[Message] = &[
    ("Usage:", "用法："),
    ("Commands:", "命令："),
    ("Arguments:", "参数："),
    ("Options:", "选项："),
    ("[OPTIONS]", "[选项]"),
    ("<COMMAND>", "<命令>"),
    ("[COMMAND]", "[命令]"),
    (
        "Print help (see more with '--help')",
        "显示帮助（用 --help 查看完整帮助）",
    ),
    (
        "Print help (see a summary with '-h')",
        "显示帮助（用 -h 查看摘要）",
    ),
    ("Print help for the subcommand(s)", "显示指定子命令的帮助"),
    ("Print help", "显示帮助"),
    ("Print version", "显示版本"),
    (
        "Print this message or the help of the given subcommand(s)",
        "显示此帮助或指定子命令的帮助",
    ),
    ("[possible values: ", "[可选值："),
];

pub const ARG_ERROR_HINT: &str = "使用 --help 查看用法。";
pub const ARG_UNKNOWN: &str = "无法识别此参数";
pub const SUBCOMMAND_UNKNOWN: &str = "无法识别此子命令";
pub const ARG_CONFLICT: &str = "这些参数不能一起使用";
pub const ARG_REQUIRED: &str = "缺少必填参数";
pub const SUBCOMMAND_REQUIRED: &str = "缺少子命令";
pub const ARG_COUNT: &str = "参数值数量有误";
pub const ARG_EQUALS: &str = "请用等号连接选项和参数值";
pub const ARG_VALUE: &str = "参数值无效";
pub const ARG_INVALID: &str = "无法解析命令行参数";
pub const ERROR_LABEL: &str = "错误";
pub const DATA_NOT_FOUND: &str = "找不到请求的记录、文件或 UClip 数据库。";
pub const SERVICE_UNAVAILABLE: &str = "本地服务不可用，请重试。";

pub const SYNC_OFF: &str = "关闭";
pub const SYNC_DISCOVERING: &str = "正在发现设备";
pub const SYNC_HOST: &str = "主机";
pub const SYNC_CLIENT: &str = "客户端";
pub const SYNC_NO_ERROR: &str = "无";
pub const SYNC_ROLE_LABEL: &str = "局域网";
pub const SYNC_PEERS_LABEL: &str = "对等设备数";
pub const SYNC_LAN_ERROR_LABEL: &str = "局域网错误";
pub const SYNC_REMOTE_ERROR_LABEL: &str = "远端错误";
