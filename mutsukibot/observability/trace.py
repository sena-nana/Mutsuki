"""结构化 trace 写入器 —— 订阅 bus 上的 ``trace.span`` 事件。"""

from __future__ import annotations

from collections.abc import Callable
import io
import json
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from mutsukibot.contracts.event import TraceSpan
    from mutsukibot.core.bus import Bus


class JsonlTraceWriter:
    """每个 span 一行 JSON 写到文件（追加模式）。

    ``attach`` 时打开文件并保持句柄；``detach`` 关闭。写失败被吞掉转发到
    bus 上的 ``trace.write_failed`` 事件，避免拖累发布者（hard rule #8 要求
    错误是结构化数据，不允许默默吞错）。
    """

    def __init__(self, path: Path | str) -> None:
        self._path = Path(path)
        self._unsubscribe: Callable[[], None] | None = None
        self._file: io.TextIOBase | None = None
        self._bus: "Bus | None" = None

    def attach(self, bus: "Bus") -> None:
        self._file = self._path.open("a", encoding="utf-8")
        self._bus = bus

        async def _on_span(payload: object) -> None:
            try:
                self._write(payload)  # type: ignore[arg-type]
            except Exception as exc:  # 不让 trace 写失败拖垮 publisher
                await bus.publish(
                    "trace.write_failed",
                    {
                        "path": str(self._path),
                        "exception_type": type(exc).__qualname__,
                        "exception_repr": repr(exc),
                    },
                )

        self._unsubscribe = bus.subscribe("trace.span", _on_span)

    def detach(self) -> None:
        if self._unsubscribe is not None:
            self._unsubscribe()
            self._unsubscribe = None
        if self._file is not None:
            self._file.close()
            self._file = None
        self._bus = None

    def _write(self, span: "TraceSpan") -> None:
        if self._file is None:
            raise RuntimeError("JsonlTraceWriter 未 attach 或已 detach")
        record = {
            "trace_id": span.trace_id,
            "span_id": span.span_id,
            "parent_span_id": span.parent_span_id,
            "name": span.name,
            "start": span.start,
            "end": span.end,
            "status": span.status.value,
            "attributes": span.attributes,
        }
        self._file.write(json.dumps(record, ensure_ascii=False) + "\n")
        self._file.flush()


__all__ = ["JsonlTraceWriter"]
