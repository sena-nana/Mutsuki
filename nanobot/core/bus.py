"""带快路径直调的内部事件总线。

订阅按事件 ``type``（字符串）分桶，并按 ``direct`` / ``deferred`` 进一步
拆成两个独立 list，避免每次 publish 重新过滤：

* **direct**：handler 由发布者内联 await（用于延迟敏感链路，如
  Yume 的 ``thought → kernel → runtime``），按注册顺序串行执行。
* **deferred**（默认）：handler 通过 :func:`asyncio.gather` 并发执行。

订阅返回一个清理回调；插件必须把它登记到自己的 :class:`PluginScope`，
这样热重载/卸载会干净地解除订阅。
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
import itertools

EventHandler = Callable[[object], Awaitable[None]]


_sub_id_counter = itertools.count(1)


@dataclass(slots=True)
class _Subscription:
    sub_id: int
    handler: EventHandler


@dataclass(slots=True)
class _EventBucket:
    direct: list[_Subscription] = field(default_factory=list)
    deferred: list[_Subscription] = field(default_factory=list)


@dataclass(slots=True)
class Bus:
    _subs: dict[str, _EventBucket] = field(default_factory=dict)

    def subscribe(
        self, event_type: str, handler: EventHandler, *, direct: bool = False
    ) -> Callable[[], None]:
        sub = _Subscription(sub_id=next(_sub_id_counter), handler=handler)
        bucket = self._subs.setdefault(event_type, _EventBucket())
        target = bucket.direct if direct else bucket.deferred
        target.append(sub)

        def _unsubscribe() -> None:
            current = self._subs.get(event_type)
            if current is None:
                return
            for lst in (current.direct, current.deferred):
                for i, s in enumerate(lst):
                    if s.sub_id == sub.sub_id:
                        del lst[i]
                        return

        return _unsubscribe

    async def publish(self, event_type: str, payload: object) -> None:
        bucket = self._subs.get(event_type)
        if bucket is None:
            return
        for sub in bucket.direct:
            await sub.handler(payload)
        if bucket.deferred:
            await asyncio.gather(*(s.handler(payload) for s in bucket.deferred))


__all__ = ["Bus", "EventHandler"]
