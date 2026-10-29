# 第三方组件与许可

JoyManga 以 [MIT License](LICENSE) 开源。发布产物（APK）内编译了以下第三方组件，其版权与许可条款如下。

## opencc-fmmseg

- 用途：搜索关键词的简繁转换
- 版本：0.13.0
- 代码许可：MIT
- 版权：Copyright (c) 2025 Laisuk Lai
- 许可全文：[third_party/opencc-fmmseg-LICENSE.txt](third_party/opencc-fmmseg-LICENSE.txt)

## OpenCC 词典（由 opencc-fmmseg 内嵌）

- 用途：简繁转换所需的字词表，已预编译进应用二进制
- 上游：Open Chinese Convert (OpenCC)
- 许可：Apache License 2.0
- 归属：Copyright © OpenCC contributors
- 许可全文：[third_party/opencc-dictionaries-LICENSE.txt](third_party/opencc-dictionaries-LICENSE.txt)

> opencc-fmmseg 可能对词典内容做过滤、合并或预编译（CBOR/Zstd），但不改变词典的 Apache-2.0 许可。
> 应用内可在「设置 → 关于 → 开源许可」查看上述全文。
