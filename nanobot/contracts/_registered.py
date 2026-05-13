"""注册式字符串子类的共享骨架。

``CapabilityName`` / ``PermissionName`` / ``ErrorCode`` 都遵循同一模式：

* ``str`` 子类
* 进程内注册表 + owner 表
* 构造时强制要求已注册，否则抛 ``UnknownXxxError``
* 同 owner 重注册幂等；跨 owner 抛 ``XxxConflictError``

把共性抽到 :class:`RegisteredString`，三个子类只需指定友好名词和错误类型。
PermissionName 因为额外携带 checker 函数，在子类侧自己增量扩展 ``register``。
"""

from __future__ import annotations

from typing import ClassVar, Self


class RegisteredString(str):
    """以注册表为唯一真相的 ``str`` 子类基类。

    每个具体子类必须设置：

    * ``_noun`` —— 用于错误信息的友好名词（如 ``"capability"``）。
    * ``_unknown_error`` —— 构造未注册值时抛的异常类型。
    * ``_conflict_error`` —— 跨 owner 注册同名时抛的异常类型。

    每个具体子类**自动**获得独立的 ``_registry`` / ``_owner`` 字典
    （通过 ``__init_subclass__`` 在 class 创建时分配）。
    """

    _registry: ClassVar[dict[str, "RegisteredString"]]
    _owner: ClassVar[dict[str, str]]

    _noun: ClassVar[str] = "value"
    _unknown_error: ClassVar[type[Exception]] = LookupError
    _conflict_error: ClassVar[type[Exception]] = ValueError

    def __init_subclass__(cls, **kwargs: object) -> None:
        super().__init_subclass__(**kwargs)
        # 每个具体子类拥有独立注册表，避免不同语义共用同一个全局 dict。
        if "_registry" not in cls.__dict__:
            cls._registry = {}
        if "_owner" not in cls.__dict__:
            cls._owner = {}

    def __new__(cls, value: str) -> Self:
        existing = cls._registry.get(value)
        if existing is None:
            raise cls._unknown_error(
                f"{cls._noun} {value!r} 未注册。"
                f"请先调用 {cls.__name__}.register({value!r}, declared_by=...)"
            )
        return existing  # type: ignore[return-value]

    @classmethod
    def _intern(cls, value: str, *, declared_by: str) -> Self:
        """注册并返回 interned 实例；同 owner 幂等，跨 owner 抛冲突。"""
        existing = cls._registry.get(value)
        if existing is not None:
            existing_owner = cls._owner[value]
            if existing_owner != declared_by:
                raise cls._conflict_error(
                    f"{cls._noun} {value!r} 已由 {existing_owner!r} 注册，"
                    f"{declared_by!r} 不可重复注册"
                )
            return existing  # type: ignore[return-value]
        instance = str.__new__(cls, value)
        cls._registry[value] = instance
        cls._owner[value] = declared_by
        return instance

    @classmethod
    def register(cls, name: str, *, declared_by: str) -> Self:
        """注册新的名字。子类如需附加元数据可重写此方法。"""
        return cls._intern(name, declared_by=declared_by)

    @classmethod
    def is_registered(cls, name: str) -> bool:
        return name in cls._registry

    @classmethod
    def owner_of(cls, name: str) -> str | None:
        return cls._owner.get(name)

    @classmethod
    def bootstrap_facade(
        cls,
        facade: type,
        items: dict[str, str],
        *,
        declared_by: str,
    ) -> None:
        """批量注册一组名字并 ``setattr`` 到 ``facade`` 类。

        ``items`` 是 ``{ATTR_NAME: registered_name}``。仅适用于 ``register``
        签名只需要 ``(name, declared_by=...)`` 的子类（``CapabilityName`` /
        ``ErrorCode``）。子类如有额外注册参数（如 ``PermissionName`` 需要
        ``checker``）应自行手写 bootstrap。
        """
        for attr, name in items.items():
            setattr(facade, attr, cls.register(name, declared_by=declared_by))


__all__ = ["RegisteredString"]
