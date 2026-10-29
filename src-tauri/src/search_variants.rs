//! 搜索关键词的简繁变体。
//!
//! JM 的搜索接口按字面匹配，繁体关键词搜不到简体标题（反之亦然）。这里把用户输入
//! 转换成另一种写法，由前端分别请求并合并结果。转换只做一次字符串处理，不涉及网络。

use std::sync::OnceLock;

use opencc_fmmseg::{OpenCC, OpenccConfig};

/// 一个关键词的简繁变体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeywordVariants {
    /// 用户输入的原文
    pub original: String,
    /// 另一种写法；简繁同形、纯数字、纯英文或转换失败时为 None
    pub converted: Option<String>,
}

static CONVERTER: OnceLock<Option<OpenCC>> = OnceLock::new();

fn converter() -> Option<&'static OpenCC> {
    CONVERTER
        .get_or_init(|| {
            let converter = OpenCC::new();
            // new() 在词典加载失败时会静默退化为空词典，必须显式检查，
            // 否则搜索会悄悄变成"只有原文变体"而没有任何提示。
            if let Some(error) = OpenCC::get_last_error() {
                tracing::warn!(error = %error, "简繁词典加载失败，搜索将只使用原文关键词");
                return None;
            }
            Some(converter)
        })
        .as_ref()
}

/// 判断字符是否属于汉字区（含扩展 A/B 与兼容区）。
fn is_han(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F
    )
}

/// 生成关键词的简繁变体。
///
/// 方向判断：先转成简体；若与输入不同，说明输入含繁体字，简体就是另一个变体；
/// 否则输入已是简体，再转成繁体。这样"输入繁体"走 t2s（歧义小），"输入简体"
/// 走 s2t，两个方向都能覆盖。
pub fn keyword_variants(keyword: &str) -> KeywordVariants {
    let original = keyword.to_string();
    // 纯数字/英文/假名没有简繁之分，避免无谓的词典初始化
    if !keyword.chars().any(is_han) {
        return KeywordVariants {
            original,
            converted: None,
        };
    }
    let Some(converter) = converter() else {
        return KeywordVariants {
            original,
            converted: None,
        };
    };

    let simplified = converter.convert_with_config(keyword, OpenccConfig::T2s, false);
    let candidate = if simplified != keyword {
        simplified
    } else {
        converter.convert_with_config(keyword, OpenccConfig::S2t, false)
    };

    let converted = (candidate != keyword && !candidate.trim().is_empty()).then_some(candidate);
    KeywordVariants {
        original,
        converted,
    }
}

#[cfg(test)]
mod tests {
    use super::keyword_variants;

    fn converted(keyword: &str) -> Option<String> {
        keyword_variants(keyword).converted
    }

    #[test]
    fn simplified_keyword_gets_traditional_variant() {
        assert_eq!(converted("头发").as_deref(), Some("頭髮"));
        assert_eq!(converted("漫画").as_deref(), Some("漫畫"));
        assert_eq!(converted("全彩").as_deref(), Some("全綵"));
    }

    #[test]
    fn traditional_keyword_gets_simplified_variant() {
        assert_eq!(converted("頭髮").as_deref(), Some("头发"));
        assert_eq!(converted("東京").as_deref(), Some("东京"));
        assert_eq!(converted("軟體").as_deref(), Some("软体"));
    }

    #[test]
    fn identical_keyword_has_no_variant() {
        assert_eq!(converted("漫画"), Some("漫畫".to_string()));
        // 简繁同形：转换结果与原词相同
        assert_eq!(converted("不良"), None);
        assert_eq!(converted("123456"), None);
        assert_eq!(converted("ABC"), None);
        assert_eq!(converted("夏の思い出"), None);
    }

    #[test]
    fn original_is_preserved() {
        let variants = keyword_variants(" 頭髮 ");
        assert_eq!(variants.original, " 頭髮 ");
        assert!(variants.converted.is_some());
    }
}
