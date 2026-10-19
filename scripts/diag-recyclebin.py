# -*- coding: utf-8 -*-
"""回收站取证扫描（AGENTS.md 删除纪律配套工具，2026-10-19）。

解析所有盘符 $Recycle.Bin 下每个 SID 目录的 $I 索引文件，还原每一条
回收站记录的：原路径、大小、精确到秒的删除时间。用于回收站异常膨胀时
定位「谁在什么时间删了什么」——先取证、后清空。

用法（任意 Python 3.8+）：
    python scripts/diag-recyclebin.py

实现说明：$I 文件头 8 字节为版本号；v2（Win10+）格式为
size(8) + filetime(8) + namelen(4) + name(UTF-16LE)；v1（旧系统）的
文件名为固定 520 字节缓冲。$R 文件是实际内容，本脚本只读 $I 元数据，
不触碰任何回收站内容，纯只读操作。
"""
import os
import string
import struct
import sys
from collections import Counter
from datetime import datetime, timedelta


def filetime_to_dt(ft):
    """把 Windows FILETIME（1601-01-01 起 100ns 计数）转成本地 datetime。"""
    try:
        return datetime(1601, 1, 1) + timedelta(microseconds=ft / 10)
    except Exception:
        return None


def parse_i_file(path):
    """解析单个 $I 元数据文件，返回 (原路径, 字节大小, 删除时间) 或 None。"""
    try:
        with open(path, "rb") as f:
            b = f.read()
    except OSError:
        return None
    if len(b) < 24:
        return None
    ver = struct.unpack_from("<q", b, 0)[0]
    size = struct.unpack_from("<q", b, 8)[0]
    ft = struct.unpack_from("<q", b, 16)[0]
    try:
        if ver >= 2:
            nl = struct.unpack_from("<i", b, 24)[0]
            if nl <= 0 or 28 + nl * 2 > len(b):
                return None
            name = b[28:28 + nl * 2].decode("utf-16-le").rstrip("\x00")
        else:
            name = b[24:24 + 520].decode("utf-16-le").rstrip("\x00")
    except Exception:
        return None
    dt = filetime_to_dt(ft)
    return None if dt is None else (name, size, dt)


def main():
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass
    top_n = int(sys.argv[1]) if len(sys.argv) > 1 else 15

    out = []
    drives = [d for d in string.ascii_uppercase
              if os.path.isdir(d + ":\\$Recycle.Bin")]
    for drive in drives:
        root = drive + ":\\$Recycle.Bin"
        try:
            sids = os.listdir(root)
        except OSError:
            continue
        for sid in sids:
            sdir = os.path.join(root, sid)
            if not os.path.isdir(sdir):
                continue
            try:
                entries = os.listdir(sdir)
            except OSError:
                continue
            for entry in entries:
                if not entry.upper().startswith("$I"):
                    continue
                parsed = parse_i_file(os.path.join(sdir, entry))
                if parsed is not None:
                    out.append((parsed[0], parsed[1], parsed[2], drive))

    print("TOTAL_ITEMS: %d" % len(out))
    print("TOTAL_SIZE_GB: %.2f" % (sum(x[1] for x in out) / 1024 ** 3))
    print()
    print("== 原路径父目录 Top %d ==" % top_n)
    for d, n in Counter(os.path.dirname(p) for p, _, _, _ in out).most_common(top_n):
        print("%d\t%s" % (n, d))
    print()
    print("== 删除日期 Top %d ==" % top_n)
    for h, n in Counter(t.strftime("%Y-%m-%d") for _, _, t, _ in out).most_common(top_n):
        print("%d\t%s" % (n, h))
    print()
    print("== 删除小时 Top %d ==" % top_n)
    for h, n in Counter(t.strftime("%Y-%m-%d %H") for _, _, t, _ in out).most_common(top_n):
        print("%d\t%s" % (n, h))
    print()
    print("== 最近 10 条 ==")
    for p, s, t, dr in sorted(out, key=lambda x: x[2], reverse=True)[:10]:
        print(dr, t.strftime("%Y-%m-%d %H:%M:%S"), "%.2fMB" % (s / 1048576), p)
    print()
    print("== 最早 5 条 ==")
    for p, s, t, dr in sorted(out, key=lambda x: x[2])[:5]:
        print(dr, t.strftime("%Y-%m-%d %H:%M:%S"), "%.2fMB" % (s / 1048576), p)


if __name__ == "__main__":
    main()
