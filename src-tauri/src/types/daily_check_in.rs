use serde::{Deserialize, Serialize};
use specta::Type;

use crate::responses::{DailyCheckInRecordDay, DailyCheckInRespData};

/// 签到面板：界面直接渲染 `record`，另外附带两个统计量，
/// 避免前端为「已签/应签」再遍历一次二维数组。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DailyCheckInState {
    pub daily_id: i64,
    pub event_name: String,
    /// 3 天/7 天奖励的进度，接口原样返回（如 `"42.86%"`）
    pub current_progress: String,
    pub three_days_coin: String,
    pub three_days_exp: String,
    pub seven_days_coin: String,
    pub seven_days_exp: String,
    /// 按周分组的当月日历；`signed` 为 `null` 表示未来日期
    pub record: Vec<Vec<DailyCheckInRecordDay>>,
    /// 到「今天」为止已经签到的天数
    pub signed_days: i64,
    /// 到「今天」为止应当签到的天数（已签 + 漏签），由接口给出的非 null 天数推出
    pub elapsed_days: i64,
}

impl DailyCheckInState {
    pub fn from_resp_data(resp: DailyCheckInRespData) -> Self {
        let days = resp.record.iter().flatten();
        let mut signed_days = 0;
        let mut elapsed_days = 0;
        for day in days {
            if day.signed.is_some() {
                elapsed_days += 1;
                if day.signed == Some(true) {
                    signed_days += 1;
                }
            }
        }
        Self {
            daily_id: resp.daily_id,
            event_name: resp.event_name,
            current_progress: resp.current_progress,
            three_days_coin: resp.three_days_coin,
            three_days_exp: resp.three_days_exp,
            seven_days_coin: resp.seven_days_coin,
            seven_days_exp: resp.seven_days_exp,
            record: resp.record,
            signed_days,
            elapsed_days,
        }
    }
}

/// 一次签到的结果。成败以「签到前后已签天数是否增加」为准，
/// 服务端文案只作提示——官方 App 靠 `msg.includes(...)` 判断，文案一改就误判。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DailyCheckInOutcome {
    /// 本次是否真的新增了一次签到（已签过则为 false）
    pub just_signed: bool,
    pub message: String,
    pub state: DailyCheckInState,
}

#[cfg(test)]
mod tests {
    use super::DailyCheckInState;

    fn parse(json: &str) -> DailyCheckInState {
        DailyCheckInState::from_resp_data(serde_json::from_str(json).unwrap())
    }

    #[test]
    fn counts_signed_and_elapsed_days_ignoring_future_days() {
        let state = parse(
            r#"{"daily_id":3,"record":[[
                {"date":"1","signed":true,"bonus":false},
                {"date":"2","signed":false,"bonus":false},
                {"date":"3","signed":true,"bonus":true},
                {"date":"4","signed":null,"bonus":false}
            ]]}"#,
        );
        assert_eq!(state.signed_days, 2);
        // 第 4 天是未来日期，不计入「应签」
        assert_eq!(state.elapsed_days, 3);
        assert_eq!(state.daily_id, 3);
    }

    #[test]
    fn empty_record_counts_zero() {
        let state = parse(r#"{"daily_id":0}"#);
        assert_eq!(state.daily_id, 0);
        assert_eq!(state.signed_days, 0);
        assert_eq!(state.elapsed_days, 0);
        assert!(state.record.is_empty());
    }

    #[test]
    fn weeks_are_flattened_across_boundaries() {
        let state = parse(
            r#"{"daily_id":1,"record":[
                [{"date":"29","signed":true},{"date":"30","signed":true}],
                [{"date":"31","signed":false}]
            ]}"#,
        );
        assert_eq!(state.signed_days, 2);
        assert_eq!(state.elapsed_days, 3);
        assert_eq!(state.record.len(), 2);
    }
}
