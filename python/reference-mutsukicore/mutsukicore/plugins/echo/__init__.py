"""Echo 参考插件 —— 同时充当 v0.1 的 LLM tool 范例。

展示 MutsukiCore 插件的标准形态：

* 类式定义，manifest 字段以 ``ClassVar`` 声明
* 嵌套 ``Config(msgspec.Struct)``
* ``@command`` 装饰的方法，docstring 驱动 tool schema 描述
"""

from typing import Annotated, ClassVar

import msgspec

from mutsukicore import Capability, Caps, Perms, Plugin, operation
from mutsukicore.contracts import Arg


class _EchoConfig(msgspec.Struct, kw_only=True):
    prefix: str = "echo: "


class EchoPlugin(Plugin[_EchoConfig]):
    """回显输入文本。展示标准插件形态。"""

    id: ClassVar[str] = "mutsukicore-echo"
    version: ClassVar[str] = "0.1.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _EchoConfig

    @operation(perms=Perms.PUBLIC)
    async def echo(
        self,
        text: str,
        count: Annotated[int, Arg(ge=1, le=10)] = 1,
    ) -> str:
        """回显输入文本。

        Args:
            text: 要回显的文本。
            count: 重复次数（1–10）。
        """
        return (self.config.prefix + text + "\n") * count


__all__ = ["EchoPlugin"]
