//! TLS 根证书策略。

/// 创建使用应用默认根证书配置的 HTTP 客户端构建器。
///
/// reqwest 0.13 的 `rustls` feature 默认使用 rustls-platform-verifier。Android 上该
/// 校验器还要求应用额外打包 Kotlin 组件并在运行时初始化，否则第一次 HTTPS 请求会在
/// `global()` 处 panic。Android 端没有打包这些组件，因此改用内置的 Mozilla 根证书，
/// 与升级前 reqwest 0.12 的 `rustls-tls`（webpki-roots）行为一致；桌面端仍使用系统
/// 证书库，以便跟随操作系统的根证书更新。
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    #[cfg(target_os = "android")]
    {
        let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().map(|der| {
            reqwest::Certificate::from_der(der.as_ref())
                .expect("内置的 Mozilla 根证书必须是合法的 DER 编码")
        });
        reqwest::ClientBuilder::new().tls_certs_only(roots)
    }
    #[cfg(not(target_os = "android"))]
    {
        reqwest::ClientBuilder::new()
    }
}
