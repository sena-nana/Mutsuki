"""MutsukiBot 契约的类型化 ID 别名。

所有 ID 在类型层面都是不透明字符串（``NewType``），这样 pyright 会标记
跨域误赋值（例如把 ``MessageId`` 传到期望 ``TraceId`` 的位置）。

``MessageId`` 是 ``EnvelopeId`` 的别名 —— Message 本身就是 Envelope 的特化
（参见 contracts §16），二者标识空间统一。
"""

from typing import NewType

AgentId = NewType("AgentId", str)
TraceId = NewType("TraceId", str)
SpanId = NewType("SpanId", str)
RefId = NewType("RefId", str)
EnvelopeId = NewType("EnvelopeId", str)
MessageId = EnvelopeId

__all__ = ["AgentId", "EnvelopeId", "MessageId", "RefId", "SpanId", "TraceId"]
