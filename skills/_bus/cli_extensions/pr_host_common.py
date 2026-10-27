"""`pr_host` 与 `pr_host_codeup` 共用的类型。

单独成文件是为了让两边导入的是同一个 `HostError`：`pr_host.py` 作为脚本运行时是 `__main__`，
若 Codeup 实现反过来 `import pr_host`，会得到第二份模块和第二个异常类，`except` 接不住。
"""

from __future__ import annotations

from dataclasses import dataclass


class HostError(RuntimeError):
    """平台不支持、命令失败或平台返回的结构不足以回答问题。"""


@dataclass(frozen=True)
class Remote:
    host: str
    path: str
    url: str
