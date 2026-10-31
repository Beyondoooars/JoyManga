use tauri::AppHandle;

use crate::errors::{CommandError, CommandResult};
use crate::extensions::AppHandleExt;
use crate::responses::{GetUserProfileRespData, GetWeeklyInfoRespData, LoginRespData, SearchResp};
use crate::types::{
    Comic, DailyCheckInOutcome, DailyCheckInState, FavoriteSort, GetFavoriteResult,
    GetWeeklyResult, SearchDualResult, SearchResultVariant, SearchSort,
};
use crate::utils;

#[tauri::command]
#[specta::specta]
pub async fn login(
    app: AppHandle,
    username: String,
    password: String,
) -> CommandResult<LoginRespData> {
    let jm_client = app.get_jm_client();

    let user_profile = jm_client
        .login(&username, &password)
        .await
        .map_err(|err| CommandError::from("登录失败", err))?;

    Ok(user_profile)
}

#[tauri::command]
#[specta::specta]
pub async fn get_user_profile(app: AppHandle) -> CommandResult<GetUserProfileRespData> {
    let jm_client = app.get_jm_client();

    let user_profile = jm_client
        .get_user_profile()
        .await
        .map_err(|err| CommandError::from("获取用户信息失败", err))?;

    Ok(user_profile)
}

#[tauri::command]
#[specta::specta]
pub async fn get_daily_check_in(app: AppHandle) -> CommandResult<DailyCheckInState> {
    app.get_jm_client()
        .get_daily_check_in()
        .await
        .map_err(|err| CommandError::from("获取签到状态失败", err))
}

#[tauri::command]
#[specta::specta]
pub async fn daily_check_in(
    app: AppHandle,
    session_epoch: u64,
) -> CommandResult<DailyCheckInOutcome> {
    app.get_jm_client()
        .daily_check_in(session_epoch)
        .await
        .map_err(|err| CommandError::from("签到失败", err))
}

/// 简繁双变体取页。
///
/// 前端按“合并流”自行决定每个变体下一次要取哪一页：两个变体的游标可以不同步，
/// 所以页码由调用方分别给出，`None` 表示本轮不取该变体。转换关键词在服务端计算，
/// 前端只拿到 `converted_keyword` 用于展示和后续游标。
#[tauri::command]
#[specta::specta]
pub async fn search_dual(
    app: AppHandle,
    keyword: String,
    sort: SearchSort,
    original_page: Option<i64>,
    converted_page: Option<i64>,
) -> CommandResult<SearchDualResult> {
    let keyword = keyword.trim().to_string();
    if keyword.is_empty() {
        return Err(CommandError::from(
            "搜索失败",
            anyhow::anyhow!("关键词不能为空"),
        ));
    }

    let variants = crate::search_variants::keyword_variants(&keyword);
    let converted_keyword = variants.converted;
    let mut result = SearchDualResult::empty(converted_keyword.clone());

    if original_page.is_none() && converted_page.is_none() {
        return Ok(result);
    }

    let id_to_dir_map =
        utils::create_id_to_dir_map(&app).map_err(|err| CommandError::from("搜索失败", err))?;
    let jm_client = app.get_jm_client();

    let original_page = original_page.map(|page| page.max(1));
    let converted_page = converted_page.map(|page| page.max(1));

    let original_future = async {
        match original_page {
            Some(page) => Some(jm_client.search(&keyword, page, sort.clone()).await),
            None => None,
        }
    };
    let converted_future = async {
        match (&converted_keyword, converted_page) {
            (Some(converted), Some(page)) => {
                Some(jm_client.search(converted, page, sort.clone()).await)
            }
            _ => None,
        }
    };
    let (original_resp, converted_resp) = tokio::join!(original_future, converted_future);

    if let Some(resp) = original_resp {
        match resp.and_then(|resp| {
            SearchResultVariant::from_search_resp_with_map(
                &app,
                resp,
                original_page.unwrap_or(1),
                &keyword,
                &id_to_dir_map,
            )
        }) {
            Ok(variant) => result.original = Some(variant),
            Err(err) => result.original_error = Some(format!("{err:#}")),
        }
    }

    if let Some(resp) = converted_resp {
        // 转换变体的跳转没有意义：只有用户原文是 JM 号时才该跳转，
        // 而纯数字关键词不会产生转换变体。返回 Comic 变体让前端把该流标记为
        // 已结束（前端不会对 converted 的 Comic 触发跳转），而不是留下 null
        // 被前端误判成“服务端未返回结果”。
        let ignored_comic = matches!(&resp, Ok(SearchResp::ComicRespData(_)));
        match resp {
            Ok(resp) => {
                let keyword_used = converted_keyword.as_deref().unwrap_or_default();
                match SearchResultVariant::from_search_resp_with_map(
                    &app,
                    resp,
                    converted_page.unwrap_or(1),
                    keyword_used,
                    &id_to_dir_map,
                ) {
                    Ok(variant) => {
                        if ignored_comic {
                            result
                                .warnings
                                .push("转换后的关键词被识别为漫画编号，已忽略".to_string());
                        }
                        result.converted = Some(variant);
                    }
                    Err(err) => result.converted_error = Some(format!("{err:#}")),
                }
            }
            Err(err) => result.converted_error = Some(format!("{err:#}")),
        }
    }

    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub async fn get_comic(app: AppHandle, aid: i64) -> CommandResult<Comic> {
    let comic = utils::get_comic(app.clone(), aid)
        .await
        .map_err(|err| CommandError::from(&format!("获取ID为`{aid}`的漫画信息失败"), err))?;

    Ok(comic)
}

#[tauri::command(async)]
#[specta::specta]
pub async fn get_favorite_folder(
    app: AppHandle,
    folder_id: i64,
    page: i64,
    sort: FavoriteSort,
) -> CommandResult<GetFavoriteResult> {
    let jm_client = app.get_jm_client();

    let get_favorite_resp_data = jm_client
        .get_favorite_folder(folder_id, page, sort)
        .await
        .map_err(|err| CommandError::from("获取收藏夹失败", err))?;

    let get_favorite_result = GetFavoriteResult::from_resp_data(&app, get_favorite_resp_data)
        .map_err(|err| CommandError::from("获取收藏夹失败", err))?;

    Ok(get_favorite_result)
}

#[tauri::command(async)]
#[specta::specta]
pub async fn get_weekly_info(app: AppHandle) -> CommandResult<GetWeeklyInfoRespData> {
    let jm_client = app.get_jm_client();

    let weekly_info = jm_client
        .get_weekly_info()
        .await
        .map_err(|err| CommandError::from("获取每周必看信息失败", err))?;

    Ok(weekly_info)
}

#[tauri::command(async)]
#[specta::specta]
pub async fn get_weekly(
    app: AppHandle,
    category_id: String,
    type_id: String,
) -> CommandResult<GetWeeklyResult> {
    let jm_client = app.get_jm_client();

    let get_weekly_resp_data = jm_client
        .get_weekly(&category_id, &type_id)
        .await
        .map_err(|err| CommandError::from("获取每周必看失败", err))?;

    let get_weekly_result = GetWeeklyResult::from_resp_data(&app, get_weekly_resp_data)
        .map_err(|err| CommandError::from("获取每周必看失败", err))?;

    Ok(get_weekly_result)
}

#[tauri::command(async)]
#[specta::specta]
pub async fn sync_favorite_folder(app: AppHandle) -> CommandResult<()> {
    app.get_jm_client()
        .get_favorite_folder(0, 1, FavoriteSort::FavoriteTime)
        .await
        .map(|_| ())
        .map_err(|err| CommandError::from("刷新收藏夹失败", err))
}

#[tauri::command(async)]
#[specta::specta]
pub async fn set_favorite_comic(
    app: AppHandle,
    comic_id: i64,
    desired: bool,
    session_epoch: u64,
) -> CommandResult<crate::events::FavoriteChangedEvent> {
    app.get_jm_client()
        .set_favorite(comic_id, desired, session_epoch)
        .await
        .map_err(|err| CommandError::from("收藏操作失败", err))
}

#[tauri::command]
#[specta::specta]
pub fn get_session(app: AppHandle) -> crate::events::SessionChangedEvent {
    app.get_jm_client().session_snapshot()
}

#[tauri::command]
#[specta::specta]
pub fn cancel_login(app: AppHandle) {
    app.get_jm_client().cancel_login();
}

#[tauri::command(async)]
#[specta::specta]
pub fn logout(app: AppHandle) -> CommandResult<()> {
    app.get_jm_client()
        .logout()
        .map_err(|err| CommandError::from("退出会话已生效，保存设置失败", err))
}

#[tauri::command(async)]
#[specta::specta]
pub async fn check_network(
    app: AppHandle,
    config: crate::config::Config,
    comic_id: Option<i64>,
) -> CommandResult<crate::network_check::NetworkCheckReport> {
    let (domain_pool_count, domain_pool_updated_at) = app.get_jm_client().domain_pool_status();
    // 诊断应与正常请求使用同一个（可能已被官方升级的）接口版本号
    let app_version = app.get_jm_client().current_app_version();
    let domain_health = app
        .get_jm_client()
        .domain_health_status(&config.get_api_domain());
    crate::network_check::check(
        config,
        comic_id,
        domain_pool_count,
        domain_pool_updated_at,
        &app_version,
        domain_health,
    )
    .await
    .map_err(|error| CommandError::from("连接检查失败", error))
}

#[tauri::command(async)]
#[specta::specta]
pub async fn get_api_domain_status(
    app: AppHandle,
) -> CommandResult<crate::network_check::NetworkProbe> {
    let client = app.get_jm_client();
    let (domain_pool_count, domain_pool_updated_at) = client.domain_pool_status();
    let current_api_domain = app.get_config().read().get_api_domain();
    let (failures, cooling_down) = client.domain_health_status(&current_api_domain);
    Ok(crate::network_check::domain_probe(
        &current_api_domain,
        domain_pool_count,
        domain_pool_updated_at,
        failures,
        cooling_down,
    ))
}

#[tauri::command]
#[specta::specta]
pub fn get_image_domain_presets() -> Vec<crate::config::ImageDomainPreset> {
    crate::config::image_domain_presets()
}
