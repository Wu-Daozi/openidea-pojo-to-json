<div align="center">

# POJO to JSON

为 [OpenIDEA](https://github.com/l0ng-ai/openidea) 打造的 Java POJO → JSON 骨架插件。

[![OpenIDEA Plugin API](https://img.shields.io/badge/OpenIDEA%20Plugin%20API-1.8-1e88e5)](https://openidea.sh/zh/plugins)
[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.2.1-green)](plugin.toml)

在 Java 类中右键 **Copy JSON**，立即得到一份带类型默认值、遵循 Jackson 注解的 JSON 骨架。

</div>

> [!NOTE]
> OpenIDEA 的插件系统仍处于 Preview 阶段，插件 API 与行为可能随 IDE 版本演进。
> 当前版本面向 **OpenIDEA 0.1.45+ / Plugin API 1.8**。

## 目录

- [为什么使用](#为什么使用)
- [功能](#功能)
- [安装](#安装)
- [使用](#使用)
- [效果示例](#效果示例)
- [默认值](#默认值)
- [设置](#设置)
- [Jackson 注解](#jackson-注解)
- [类型解析](#类型解析)
- [时区](#时区)
- [权限与安全](#权限与安全)
- [兼容性](#兼容性)
- [已知限制](#已知限制)
- [开发](#开发)
- [项目结构](#项目结构)
- [测试](#测试)
- [发布插件包](#发布插件包)
- [贡献](#贡献)
- [更新记录](#更新记录)
- [致谢](#致谢)
- [License](#license)

## 为什么使用

手写测试 JSON、接口 Mock 数据或请求体时，重复劳动多，而且容易漏字段、写错类型、忘记 Jackson 的重命名规则。

这个插件直接读取当前 Java 源码（包括未保存的修改），按字段声明顺序生成 JSON：

- 不需要运行项目，不需要反射，不依赖编译产物。
- 支持 Lombok 普通字段、record、嵌套类、继承字段。
- 支持常用 Jackson 字段/类级注解。
- 只读编辑器与项目源码，只写系统剪贴板。

## 功能

- **右键即用**：Java 编辑器右键菜单 **Copy JSON**，也可以在 **查找操作** 中执行 `POJO to JSON: Copy JSON`。
- **class 与 record**：解析当前光标所在的最内层类；支持嵌套类和多个顶层类的选择器。
- **继承字段**：父类字段排在子类之前，子类同名字段覆盖父类。
- **跨文件类型**：按 import、package、项目符号索引解析同项目源码中的类型。
- **集合示例**：`List<Pojo>` / `Pojo[]` 生成一个示例对象；`List<Enum>` 生成第一个枚举常量。
- **Jackson 支持**：`@JsonProperty`、`@JsonIgnore`、`@JsonNaming`、`@JsonIgnoreProperties`、`@JsonProperty(access=...)`、`@JsonIgnoreType`。
- **当前时间**：日期时间字段使用执行时的当前时间，一次复制内所有时间字段保持一致。
- **可配置**：命名策略、static/transient、继承、未知类型、递归深度、缩进、预览等。
- **复制前预览**：开启 `previewBeforeCopy` 后可以先编辑 JSON，再写入剪贴板。
- **安全**：不写项目文件、不联网、不启动进程；权限仅为读取编辑器和项目源码、写剪贴板。

## 安装

### 方式一：从插件市场安装

插件上架后，在 **设置 › 插件 › 插件市场** 中搜索 **POJO to JSON**，点击安装并授予所需权限即可。

如果市场暂未显示，可以使用下面两种方式。

### 方式二：安装 `.oiplugin` 插件包

以下命令需要先安装 Rust 与 `openidea-plugin`，步骤见 [开发 › 安装工具链](#开发)。

先从源码构建插件包：

```sh
git clone https://github.com/Wu-Daozi/openidea-pojo-to-json.git
cd openidea-pojo-to-json
openidea-plugin build
openidea-plugin pack --out dist/wudaozi.pojo-to-json-0.2.1.oiplugin
```

然后在 OpenIDEA 中：

1. 打开 **设置 › 插件**。
2. 点击齿轮菜单，选择 **从插件包文件安装…**。
3. 选择 `dist/wudaozi.pojo-to-json-0.2.1.oiplugin`。
4. 按提示授予 `editor.read`、`clipboard`、`project.read`。

未签名的插件包在安装时会显示安全提示；正式分发前请使用发布者密钥签名。

### 方式三：链接开发中的插件目录

适合修改源码后快速验证：

```sh
git clone https://github.com/Wu-Daozi/openidea-pojo-to-json.git
cd openidea-pojo-to-json
openidea-plugin build
openidea-plugin link
```

然后打开 **设置 › 插件**，找到 **POJO to JSON**，点击 **从磁盘重新加载**。
之后每次修改源码，重新执行 `openidea-plugin build` 并在插件详情中重载即可。

如果使用自定义插件目录，请确认 `OPENIDEA_PLUGINS_DIR` 指向你正在运行的 IDE 的插件目录。

## 使用

1. 打开任意 `.java` 文件。
2. 把光标放到类或 record 内部。
3. 右键选择 **Copy JSON**。
4. 粘贴到 JSON 文件、接口调试工具或测试数据中。

说明：

- 文件里有多个顶层类且光标不在任何类内时，会弹出类名选择器。
- 插件不声明默认快捷键；需要快捷键时，在 **设置 › 按键映射** 中搜索 `POJO to JSON: Copy JSON` 自行绑定。
- 菜单按语言显示，因此会出现在所有 Java 文件中；实际复制时再根据光标定位目标类。

## 效果示例

```java
public class User {
    private Long id;
    private String nickName;
    private boolean active;
    private List<String> tags;
    private LocalDate birthday;
    private Status status;
    private Address address;

    private static final long serialVersionUID = 1L;

    @JsonIgnore
    private String password;

    @JsonProperty("display_name")
    private String displayName;

    static class Address {
        private String city;
    }

    enum Status { ACTIVE, DISABLED }
}
```

复制结果：

```json
{
  "id": 0,
  "nickName": "",
  "active": false,
  "tags": [],
  "birthday": "2026-11-18",
  "status": "ACTIVE",
  "address": {
    "city": ""
  },
  "display_name": ""
}
```

日期时间字段会替换为执行时的当前时间；上例中的日期仅用于展示格式。

## 默认值

| Java 类型 | JSON 默认值 |
| --- | --- |
| `boolean` / `Boolean` / `AtomicBoolean` | `false` |
| `byte` `short` `int` `long` 及包装类、`BigInteger` `BigDecimal` `Number` `AtomicInteger` `AtomicLong` | `0` |
| `float` `double` / `Float` `Double` | `0.0` |
| `char` / `Character` | `""` |
| `String` `CharSequence` `StringBuilder` `StringBuffer` | `""` |
| `UUID` | `"00000000-0000-0000-0000-000000000000"` |
| `LocalDate` | 当前日期，`yyyy-MM-dd` |
| `LocalTime` / `Time` | 当前时间，`HH:mm:ss` |
| `LocalDateTime` / `Date` / `Timestamp` / `Instant` / `ZonedDateTime` / `OffsetDateTime` | 当前时间，`yyyy-MM-dd HH:mm:ss` |
| `Duration` / `Period` | `"PT0S"` / `"P0D"` |
| 数组、`List` `Set` `Collection` `Iterable` `Queue` `Deque`（元素为标量/未知类型） | `[]` |
| `List<Pojo>` / `Pojo[]`（Pojo 可解析） | `[ { ... } ]`，一个示例对象 |
| `List<Enum>` | `["第一个枚举常量"]` |
| `Map` / `Properties` 等 | `{}` |
| `Optional` / `OptionalInt` / `OptionalLong` / `OptionalDouble` | `null` |
| 可解析的 enum | 第一个枚举常量名 |
| 可解析的 class/record | 递归生成嵌套对象 |
| 未知类型、`Object`、接口、类型变量 | `null`（默认；可改为 `{}`） |

补充规则：

- 默认跳过 `static` 字段；开启 `includeStatic` 后可输出，但 `serialVersionUID` 始终跳过。
- `transient` 字段默认保留，可通过 `includeTransient` 关闭。
- `@JsonIgnore`、`@JsonProperty(access = WRITE_ONLY)`、`@JsonIgnoreType` 的字段不会输出。
- 不读取字段初始化值；`private int status = 1;` 仍输出 `0`。
- 一个声明里有多个变量（`int a, b;`）时逐个输出。
- 递归深度默认 5；循环引用输出 `null`。
- `scalarCollectionSample=true` 时，`List<String>` 会输出 `[""]` 而不是 `[]`。

## 设置

在 **设置** 中搜索 **POJO to JSON**：

| 设置 | 默认值 | 说明 |
| --- | --- | --- |
| `namingStrategy` | `field` | 字段命名策略：`field`、`snake_case`、`kebab-case`、`camelCase`、`PascalCase`、`UPPER_SNAKE`；`@JsonProperty` 始终优先 |
| `includeStatic` | `false` | 是否输出 static 字段；`serialVersionUID` 始终跳过 |
| `includeTransient` | `true` | 是否输出 transient 字段 |
| `includeInherited` | `true` | 是否输出父类字段 |
| `scalarCollectionSample` | `false` | 标量集合是否生成一个示例元素 |
| `unknownType` | `null` | 未解析类型输出 `null` 或 `{}`；集合元素对应 `[]` 或 `[{}]` |
| `maxDepth` | `5` | 递归深度，1–10 |
| `indent` | `2` | JSON 缩进，2 或 4 空格 |
| `previewBeforeCopy` | `false` | 复制前打开多行编辑器预览/修改 |
| `timezoneOffsetMinutes` | `480` | 当前日期时间的 UTC 偏移（分钟） |

所有设置都只在生成阶段生效，不会修改项目源码。

## Jackson 注解

| 注解 | 行为 |
| --- | --- |
| `@JsonProperty("x")` | 覆盖 JSON 键名 |
| `@JsonIgnore` | 跳过字段 |
| `@JsonNaming(SnakeCaseStrategy.class)` | 类级命名策略；子类会作用于继承字段 |
| `@JsonIgnoreProperties({"a","b"})` | 类级排除字段；按原始属性名匹配，子类同样作用于继承字段 |
| `@JsonProperty(access = WRITE_ONLY)` | 不输出 |
| `@JsonProperty(access = READ_ONLY)` | 输出 |
| `@JsonIgnoreType` | 该类型字段从 JSON 中省略 |

说明：

- 只读取字段/record 组件上的注解，不读取 getter 上的 Jackson 注解。
- `@JsonProperty` 显式键名优先于 `@JsonNaming` 和全局 `namingStrategy`。
- `@JsonIgnoreProperties` 在命名策略之前匹配原始 Java 属性名，与 Jackson 属性收集顺序一致。
- 子类 `@JsonNaming` 和 `@JsonIgnoreProperties` 会作用于父类字段。

不支持：`@JsonUnwrapped`、`@JsonFormat`、getter 注解、运行时类型推导。

## 类型解析

外部类型按以下顺序解析：

1. 同一编译单元中的 class/record/enum。
2. 显式 `import com.example.Type` 指向的源文件。
3. 同 package 下的 `Type.java`。
4. `import com.example.*` 对应的包。
5. 项目符号索引（`Project::symbols`）。
6. 同名源文件回退。

候选文件会校验 package 和实际类型名；同一 package 下同时存在 `src/main` 与 `src/test` 时，优先 `src/main`。

解析依赖 `project.read` 权限和已打开的项目。独立打开的文件、依赖 jar 中的类、未打开的项目无法读取源码，会按 `unknownType` 回退。

## 时区

插件运行在 WebAssembly 沙箱中，无法读取宿主机的系统时区，因此日期时间使用固定 UTC 偏移，默认：

```text
timezoneOffsetMinutes = 480   # UTC+8，Asia/Shanghai
```

其他常用值：

| 时区 | 值 |
| --- | --- |
| UTC | `0` |
| UTC+8（默认） | `480` |
| UTC-5 | `-300` |

一次 Copy JSON 中所有日期时间字段使用同一个时间点。偏移量是固定值，不处理夏令时切换。

## 权限与安全

| 能力 | 风险 | 用途 |
| --- | --- | --- |
| `editor.read` | 低 | 读取当前编辑器文本、选区和路径 |
| `clipboard` | 中 | 把生成的 JSON 写入系统剪贴板 |
| `project.read` | 低 | 读取同项目源码以解析外部类型 |

插件不会：

- 写入项目文件或 IDE 配置；
- 访问网络；
- 启动外部进程；
- 上传源码或 JSON；
- 读取项目之外的文件。

插件只读取当前打开文件的文本；`project.read` 仅用于读取当前项目内的源码。

剪贴板内容可能被其他应用读取，这是剪贴板能力本身的属性；插件只在用户执行 **Copy JSON** 时写入。

## 兼容性

| 项目 | 值 |
| --- | --- |
| 插件 ID | `wudaozi.pojo-to-json` |
| OpenIDEA | 0.1.45+（Plugin API 1.8） |
| 插件 API | `engines.openidea = "^1.8"` |
| 插件版本 | 0.2.1 |
| 目标 | `wasm32-wasip2` |
| Rust | 1.85+，edition 2024 |
| 许可证 | Apache-2.0 |

插件 API 的 1.x 版本只增不改；为 1.8 构建的组件可在后续 1.x IDE 中继续加载。

## 已知限制

- 只读取字段/record 组件上的 Jackson 注解，getter 注解不生效。
- 不支持 `@JsonUnwrapped`、`@JsonFormat`。
- 依赖 jar、classpath 中的类型没有源码可读，无法展开字段。
- 外部类型按源码路径和包名解析，不等价于编译器的完整 classpath 解析。
- 独立打开、未打开项目或未授予 `project.read` 时，只能解析同文件类型。
- 菜单按语言显示，不能只对“类”显示。
- 目标文件与外部源码均限制为 2 MiB。
- 日期时间默认 UTC+8，且不处理夏令时。
- 插件不声明默认快捷键。
- 不读取字段初始化值。

## 开发

### 环境要求

- OpenIDEA 0.1.45 或更高版本。
- Rust stable（1.85+，edition 2024）。
- `wasm32-wasip2` target。
- `openidea-plugin` 工具，版本与 OpenIDEA 对齐。

### 安装工具链

```sh
rustup target add wasm32-wasip2

cargo install --locked --git https://github.com/l0ng-ai/openidea \
  --tag v0.1.45 ide-host --bin openidea-plugin

openidea-plugin --version   # 期望 plugin API 1.8
```

### 构建与链接

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test

openidea-plugin build
openidea-plugin check
openidea-plugin link
```

`openidea-plugin link` 会把插件目录写入 OpenIDEA 的 `links.json`。随后在 **设置 › 插件** 中点击 **从磁盘重新加载**。

修改 `plugin.toml` 的菜单、权限或设置 schema 后，同样需要重载插件。

### 常用命令

```sh
cargo test                                   # 单元测试 + MockHost 集成测试
openidea-plugin build                        # 构建 dist/plugin.wasm 并执行 check
openidea-plugin check                        # 校验插件包
openidea-plugin pack --out dist/wudaozi.pojo-to-json-0.2.1.oiplugin
```

## 项目结构

```text
.
├── Cargo.toml                  # Rust 包、依赖与 release profile
├── Cargo.lock                  # 依赖锁定
├── plugin.toml                 # OpenIDEA 清单：命令、菜单、设置、能力
├── settings.schema.json        # 插件设置项
├── icon.svg                    # 插件图标
├── locales/
│   └── zh-CN.json              # 中文文案
└── src/
    ├── lib.rs                  # 命令入口、项目解析、剪贴板、通知
    ├── options.rs              # 设置读取与命名策略
    ├── pojo.rs                 # Java AST、继承、Jackson、JSON 构建
    └── defaults.rs             # 类型默认值、当前日期时间
```

`dist/`、`target/` 为构建产物，不提交到 Git。

## 测试

```sh
cargo test
```

当前测试覆盖：

- class/record/嵌套类/多顶层类。
- 继承字段、子类命名与忽略。
- `@JsonProperty`、`@JsonIgnore`、`@JsonNaming`、`@JsonIgnoreProperties`、`@JsonIgnoreType`。
- 跨文件、跨 package、wildcard import 类型解析。
- 同名类、main/test 源根、错误 package、超限源码。
- 集合与未知类型、`unknownType`、递归深度、缩进。
- 预览编辑、取消、通知字段数。
- MockHost 的能力拒绝与剪贴板行为。

## 发布插件包

```sh
# 未签名，适合本地测试
openidea-plugin pack --out dist/wudaozi.pojo-to-json-0.2.1.oiplugin

# 签名分发
openidea-plugin keygen publisher.key
openidea-plugin pack \
  --key publisher.key \
  --publisher wudaozi \
  --out dist/wudaozi.pojo-to-json-0.2.1.oiplugin
```

签名前请确认插件 id 前缀与发布者 id 一致（当前为 `wudaozi.`）。

## 贡献

欢迎通过 [Issues](https://github.com/Wu-Daozi/openidea-pojo-to-json/issues) 和 [Pull Requests](https://github.com/Wu-Daozi/openidea-pojo-to-json/pulls) 参与。

提交前请确保：

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
openidea-plugin build
openidea-plugin check
```

建议：

- 行为变更请补充测试，并更新 README 与 `plugin.toml` 的 `change-notes`。
- 新增设置项或权限前先开 Issue 讨论，避免破坏现有配置兼容性。
- 不要提交 `dist/`、`target/` 或本机的 `publisher.key`。

## 更新记录

| 版本 | 内容 |
| --- | --- |
| `0.2.1` | 修复继承字段的命名/忽略顺序；外部类型使用各自文件的 import；`@JsonIgnoreType` 改为省略属性；JDK 类型短路；候选校验与预览计数修复 |
| `0.2.0` | import/package 感知解析；继承字段；Jackson 注解；可配置输出；复制前预览 |
| `0.1.0` | 首个版本：右键复制 Java POJO 的 JSON 骨架 |

## 致谢

- [OpenIDEA](https://github.com/l0ng-ai/openidea)：插件 API、SDK 和插件工具链。
- [java-lang](https://github.com/mzdk100/java-lang)：纯 Rust Java AST 解析器。
- [Jackson](https://github.com/FasterXML/jackson)：注解语义参考。

## License

本项目使用 [Apache License 2.0](LICENSE)。

---

<div align="center">

如果这个插件对你有帮助，欢迎 [Star](https://github.com/Wu-Daozi/openidea-pojo-to-json) ⭐

</div>
