use tauri::AppHandle;

use crate::errors::{CommandError, CommandResult};
use crate::extensions::AppHandleExt;
use crate::responses::{GetUserProfileRespData, GetWeeklyInfoRespData, LoginRespData};
use crate::types::{
    Comic, FavoriteSort, GetFavoriteResult, GetWeeklyResult, SearchResultVariant, SearchSort,
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
pub async fn search(
    app: AppHandle,
    keyword: String,
    page: i64,
    sort: SearchSort,
) -> CommandResult<SearchResultVariant> {
    let jm_client = app.get_jm_client();

    let search_resp = jm_client
        .search(&keyword, page, sort)
        .await
        .map_err(|err| CommandError::from("搜索失败", err))?;

    let search_result = SearchResultVariant::from_search_resp(&app, search_resp)
        .map_err(|err| CommandError::from("搜索失败", err))?;

    Ok(search_result)
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
