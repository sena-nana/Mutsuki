# 注册式字符串扩展

## 这是什么

Mutsuki 把"可扩展但要排斥拼写错误"的字符串集中处理 —— 三个子系统共用同一套 `RegisteredString` 骨架：

- `CapabilityName` —— 能力名
- `PermissionName` —— 权限名
- `ErrorCode` —— 错误码

代码：[mutsuki/contracts/_registered.py](../../mutsuki/contracts/_registered.py)。

## 解决什么问题

字符串作为标识符有两面性：扩展方便，但容易拼写错误。Mutsuki 想要：

- 第三方插件可以自由注册新名字
- 但 `CapabilityName("yume.vrma")`（拼错）必须立即报错，不是运行到那条路径才发现
- 同名不能被两个插件覆盖

`RegisteredString` 把这套语义抽到一个 `str` 子类基类里，`CapabilityName` / `PermissionName` / `ErrorCode` 只声明友好名词与异常类型即可获得：

1. 进程内全局注册表
2. 构造时强制要求已注册
3. 同 owner 重注册幂等
4. 跨 owner 注册同名抛冲突

## 怎么工作

### RegisteredString 基类

[_registered.py:19-103](../../mutsuki/contracts/_registered.py#L19-L103) 关键片段：

```python
class RegisteredString(str):
    _registry: ClassVar[dict[str, "RegisteredString"]]
    _owner: ClassVar[dict[str, str]]
    _noun: ClassVar[str] = "value"
    _unknown_error: ClassVar[type[Exception]] = LookupError
    _conflict_error: ClassVar[type[Exception]] = ValueError

    def __init_subclass__(cls, **kwargs: object) -> None:
        super().__init_subclass__(**kwargs)
        if "_registry" not in cls.__dict__:
            cls._registry = {}
        if "_owner" not in cls.__dict__:
            cls._owner = {}

    def __new__(cls, value: str) -> Self:
        existing = cls._registry.get(value)
        if existing is None:
            raise cls._unknown_error(...)
        return existing

    @classmethod
    def _intern(cls, value: str, *, declared_by: str) -> Self:
        existing = cls._registry.get(value)
        if existing is not None:
            existing_owner = cls._owner[value]
            if existing_owner != declared_by:
                raise cls._conflict_error(...)
            return existing
        instance = str.__new__(cls, value)
        cls._registry[value] = instance
        cls._owner[value] = declared_by
        return instance

    @classmethod
    def register(cls, name: str, *, declared_by: str) -> Self:
        return cls._intern(name, declared_by=declared_by)

    @classmethod
    def bootstrap_facade(cls, facade, items, *, declared_by) -> None:
        for attr, name in items.items():
            setattr(facade, attr, cls.register(name, declared_by=declared_by))
```

要点：

- **`__init_subclass__` 给每个子类独立的 `_registry` / `_owner`**——避免 `CapabilityName` 与 `ErrorCode` 共用同一字典
- **`__new__` 总是返回 interned 实例**——同一字符串注册过后，所有 `CapabilityName("read_message")` 调用拿到的是 *同一个对象*（identity 相等），可以直接用 `is` 比较
- **`_intern` 是注册的内核**——`register` 是公开门面；`bootstrap_facade` 是批量注册 + 设属性的便利方法

### 三个子类的差异

| 子类 | 额外字段 | 额外行为 |
|---|---|---|
| `CapabilityName` | 无 | 仅 `register(name, declared_by)` |
| `PermissionName` | `_checker: dict[name, CheckerFn]` | `register(name, declared_by, checker)` 多收一个 checker；`to_rule()` 取 checker 包成 `PermissionRule` |
| `ErrorCode` | 无 | 仅 `register(name, declared_by)` |

`PermissionName` 因为有 checker 没法用 `bootstrap_facade`，自己手写 `_bootstrap()`（[permission_builtin.py:38-44](../../mutsuki/contracts/permission_builtin.py#L38-L44)）。

### bootstrap_facade 模式

`Caps` / `Errs` / `Perms` 这三个门面都是"空壳类 + ClassVar 注解 + 启动时 setattr"：

```python
class Caps:
    READ_MESSAGE: ClassVar[CapabilityName]
    SEND_MESSAGE: ClassVar[CapabilityName]
    ...

CapabilityName.bootstrap_facade(
    Caps,
    {
        "READ_MESSAGE": "read_message",
        "SEND_MESSAGE": "send_message",
        ...
    },
    declared_by="mutsuki.core",
)
```

ClassVar 注解让 pyright 知道 `Caps.READ_MESSAGE` 的类型；`bootstrap_facade` 在模块导入时给它实际赋值。两层都需要 —— 单写注解 IDE 能补全但运行时 AttributeError；单写 setattr 类型推断不出来。

## 用法示例

### 给自己的能力命名空间建门面

```python
from typing import ClassVar
from mutsuki.contracts.capability import CapabilityName

class YumeCaps:
    VRAM: ClassVar[CapabilityName]
    KV_CACHE: ClassVar[CapabilityName]
    SAMPLE: ClassVar[CapabilityName]

CapabilityName.bootstrap_facade(
    YumeCaps,
    {
        "VRAM": "yume.vram",
        "KV_CACHE": "yume.kv_cache",
        "SAMPLE": "yume.sample",
    },
    declared_by="yume.runtime",
)
```

### 注册自有 PermissionName

```python
from mutsuki.contracts.permission import PermissionName

async def _is_alpha_user(ctx) -> bool:
    return ctx.message and ctx.message.source.user_id in ALPHA_ALLOWLIST

ALPHA_ONLY = PermissionName.register(
    "yume.alpha_only",
    declared_by="yume.runtime",
    checker=_is_alpha_user,
)
```

### 注册自有 ErrorCode

```python
from mutsuki.contracts.error import ErrorCode

YUME_KERNEL_TIMEOUT = ErrorCode.register(
    "yume.kernel.timeout",
    declared_by="yume.runtime",
)
```

### 校验"是否注册过"

```python
CapabilityName.is_registered("yume.vram")     # True / False
CapabilityName.owner_of("yume.vram")          # "yume.runtime" / None
```

## 常见陷阱

- **构造未注册的名字会立即抛错**——`CapabilityName("typo")` 抛 `UnknownCapabilityError`。这意味着导入时序很重要：你的扩展注册必须在第一次构造之前完成。最佳实践：把 `bootstrap_facade` 调用放到包的 `__init__.py` 里。
- **同 owner 重注册幂等**——你导两次同一个模块也不会冲突。但跨 owner 注册同名抛冲突。
- **identity 比较等于 equality**——因为同一 interned 实例，`CapabilityName("foo") is CapabilityName("foo")` 一定 True（前提是已注册）。这让 `set` 操作很快。
- **`bootstrap_facade` 不适合带额外参数的 `register`**——`PermissionName.register` 多了 checker 参数，所以 Perms 自己手写 bootstrap，没用通用方法。
- **类属性形式（`Caps.READ_MESSAGE`）与构造形式（`CapabilityName("read_message")`）等价**——返回同一对象。但 `Caps.READ_MESSAGE` 走属性访问更快、IDE 友好。
- **不要在 `ClassVar` 默认值里直接构造**——比如 `class MyPlugin(Plugin): cap: ClassVar = CapabilityName("read_message")`，import 这个模块时如果 `Caps` 还没 bootstrap 完会报 UnknownCapabilityError。建议永远用 `Caps.READ_MESSAGE`，不要绕过门面构造。
