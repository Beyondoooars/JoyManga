use serde::{Deserialize, Serialize};
use specta::Type;

use super::string_to_i64;

/// `GET /daily`：当月签到面板。
///
/// 字段集合按官方 App 与社区客户端（JMComic-Api-Java）的实现对齐：
/// 金币/经验奖励都是**字符串**（可能为空），`record` 是按周分组的当月日历。
/// 所有字段都可缺省——活动下线或接口版本变化时不应让整次请求解析失败。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct DailyCheckInRespData {
    /// 签到活动 ID，打卡时需要原样回传
    #[serde(rename = "daily_id", deserialize_with = "string_to_i64")]
    pub daily_id: i64,
    #[serde(rename = "event_name")]
    pub event_name: String,
    #[serde(rename = "three_days_coin")]
    pub three_days_coin: String,
    #[serde(rename = "three_days_exp")]
    pub three_days_exp: String,
    #[serde(rename = "seven_days_coin")]
    pub seven_days_coin: String,
    #[serde(rename = "seven_days_exp")]
    pub seven_days_exp: String,
    /// 3 天/7 天奖励的进度，形如 `"33.33%"`（接口原样返回，前端只做展示）
    pub current_progress: String,
    pub record: Vec<Vec<DailyCheckInRecordDay>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct DailyCheckInRecordDay {
    /// 当月第几天，形如 `"5"`（补零的 `"05"` 也能解析）
    pub date: String,
    /// `null` 表示未来日期（当天及以后尚未结算），`true` 已签，`false` 漏签
    pub signed: Option<bool>,
    /// 奖励日（每 3/7 天）
    pub bonus: bool,
}

/// `POST /daily_chk`：签到只有一句服务端文案，成败需由签到前后的状态推断。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct DailyCheckInResultRespData {
    pub msg: String,
}

#[cfg(test)]
mod tests {
    use super::{DailyCheckInRecordDay, DailyCheckInRespData};

    #[test]
    fn daily_status_parses_official_shape() {
        let parsed: DailyCheckInRespData = serde_json::from_str(
            r#"{
                "daily_id": 7,
                "event_name": "每日簽到",
                "three_days_coin": "5",
                "three_days_exp": "10",
                "seven_days_coin": "15",
                "seven_days_exp": "30",
                "currentProgress": "42.86%",
                "record": [[
                    {"date": "1", "signed": true, "bonus": false},
                    {"date": "2", "signed": null, "bonus": true}
                ]]
            }"#,
        )
        .unwrap();
        assert_eq!(parsed.daily_id, 7);
        assert_eq!(parsed.event_name, "每日簽到");
        assert_eq!(parsed.three_days_coin, "5");
        assert_eq!(parsed.current_progress, "42.86%");
        assert_eq!(parsed.record.len(), 1);
        assert_eq!(parsed.record[0][0].signed, Some(true));
        // 未来日期返回 null，必须与「漏签」的 false 区分开
        assert_eq!(parsed.record[0][1].signed, None);
        assert!(parsed.record[0][1].bonus);
    }

    #[test]
    fn daily_status_tolerates_string_id_and_missing_fields() {
        let parsed: DailyCheckInRespData = serde_json::from_str(r#"{"daily_id":"3"}"#).unwrap();
        assert_eq!(parsed.daily_id, 3);
        assert!(parsed.record.is_empty());
        assert!(parsed.current_progress.is_empty());
    }

    #[test]
    fn record_day_defaults_to_unsettled() {
        let day: DailyCheckInRecordDay = serde_json::from_str(r#"{"date":"9"}"#).unwrap();
        assert_eq!(day.signed, None);
        assert!(!day.bonus);
    }
}
