"""带快路径直调的内部事件总线。

订阅按事件 ``type``（字符串）分桶，并按 ``direct`` / ``deferred`` 进一步
拆成两个独立 list，避免每次 publish 重新过滤：

* **direct**：handler 由发布者内联 await（用于延迟敏感链路，如
  Yume 的 ``thought → kernel → runtime``），按注册顺序串行执行。
  失败会向 publisher 透传——direct 订阅者的契约就是"不抛"。
* **deferred**（默认）：handler 通过 :func:`asyncio.gather` 并发执行。
  失败被隔离：一个 handler 抛错不会取消其他 handler，也不会拖死 publisher。
  这是 observability 旁路订阅的物理保证（一个挂掉的 trace writer 不能
  影响主消息链路）。失败会写入 :data:`failures` 供巡检读取，并通过标准
  ``logging`` 输出。

订阅返回一个清理回调；插件必须把它登记到自己的 :class:`PluginScope`，
这样热重载/卸载会干净地解除订阅。
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
import itertools
import logging

EventHandler = Callable[[object], Awaitable[None]]

_logger = logging.getLogger(__name__)
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
class HandlerFailure:
    """deferred handler 抛错时的诊断记录。"""

    event_type: str
    sub_id: int
    exception_type: str
    exception_repr: str


@dataclass(slots=True)
class Bus:
    _subs: dict[str, _EventBucket] = field(default_factory=dict)
    failures: list[HandlerFailure] = field(default_factory=list)

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
            results = await asyncio.gather(
                *(s.handler(payload) for s in bucket.deferred),
                return_exceptions=True,
            )
            for sub, result in zip(bucket.deferred, results, strict=True):
                if isinstance(result, BaseException):
                    failure = HandlerFailure(
                        event_type=event_type,
                        sub_id=sub.sub_id,
                        exception_type=type(result).__qualname__,
                        exception_repr=repr(result),
                    )
                    self.failures.append(failure)
                    _logger.warning(
                        "bus deferred handler failed: event=%s sub_id=%d exc=%s",
                        event_type,
                        sub.sub_id,
                        failure.exception_repr,
                    )


__all__ = ["Bus", "EventHandler", "HandlerFailure"]
