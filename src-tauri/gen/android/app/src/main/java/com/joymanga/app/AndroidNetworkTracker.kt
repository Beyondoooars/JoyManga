package com.joymanga.app

import android.app.Activity
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.webkit.WebView
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap

internal class AndroidNetworkTracker(
    private val activity: Activity,
    private val webViewProvider: () -> WebView?,
) {
    private data class PhysicalNetworkInfo(val kind: String, val validated: Boolean, val notMetered: Boolean)

    /// 当前可用的物理网络（Wi-Fi/蜂窝）：VPN 开启时默认网络是隧道，
    /// 只有跟踪这一层才能发现底层换网。
    private val physicalNetworks = ConcurrentHashMap<Network, PhysicalNetworkInfo>()
    private var defaultNetworkCallback: ConnectivityManager.NetworkCallback? = null
    private var physicalNetworkCallback: ConnectivityManager.NetworkCallback? = null
    private var lastPushedMetered: Boolean? = null
    private var lastPushedVpnActive: Boolean? = null
    private var lastPushedNetworkKey: String? = null

    /**
     * 网络切换（Wi-Fi <-> 移动数据）时把"是否计费"推给前端，供「仅 Wi-Fi 下载」判断。
     *
     * 必须监听**默认网络**：按 INTERNET 能力注册回调时，Wi-Fi 与移动数据会各自
     * 触发一次，最后到达的那个说了算——Wi-Fi + 蜂窝并存（或开 VPN）时会把
     * 不计费的 Wi-Fi 误判成计费网络，导致「仅 Wi-Fi 下载」无故暂停。
     */
    fun register() {
        unregister()
        try {
            val manager = activity.getSystemService(ConnectivityManager::class.java)
            val defaultCallback = object : ConnectivityManager.NetworkCallback() {
                override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                    val metered = if (capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) {
                        effectiveMetered(manager, network)
                    } else {
                        !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
                    }
                    pushMetered(metered)
                    pushVpn(
                        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN),
                        networkPathKey(network),
                    )
                }

                override fun onLost(network: Network) {
                    pushCurrentNetworkState()
                }
            }
            defaultNetworkCallback = defaultCallback
            manager.registerDefaultNetworkCallback(defaultCallback)

            // 默认网络回调在 VPN 下只报告隧道；额外跟踪物理网络，
            // 把隧道底层的 Wi-Fi↔蜂窝切换也纳入网络路径标识。
            val physicalRequest = NetworkRequest.Builder()
                .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
                .build()
            val physicalCallback = object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    if (updatePhysicalNetwork(manager, network)) {
                        pushCurrentNetworkState()
                    }
                }

                override fun onCapabilitiesChanged(
                    network: Network,
                    capabilities: NetworkCapabilities,
                ) {
                    physicalNetworks[network] = PhysicalNetworkInfo(
                        physicalKind(capabilities),
                        capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED),
                        capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED),
                    )
                    pushCurrentNetworkState()
                }

                override fun onLost(network: Network) {
                    physicalNetworks.remove(network)
                    pushCurrentNetworkState()
                }
            }
            physicalNetworkCallback = physicalCallback
            manager.registerNetworkCallback(physicalRequest, physicalCallback)

            seedPhysicalNetworks(manager)
            pushCurrentNetworkState()
        } catch (error: Exception) {
            android.util.Log.w("JoyManga", "读取网络计费状态失败", error)
        }
    }

    fun unregister() {
        try {
            val manager = activity.getSystemService(ConnectivityManager::class.java)
            defaultNetworkCallback?.let { manager.unregisterNetworkCallback(it) }
            physicalNetworkCallback?.let { manager.unregisterNetworkCallback(it) }
        } catch (_: Exception) {}
        defaultNetworkCallback = null
        physicalNetworkCallback = null
    }

    /** 启动/换网/物理网络变化时统一推送一次当前状态。 */
    fun pushCurrentNetworkState() {
        val manager = activity.getSystemService(ConnectivityManager::class.java)
        pushMetered(effectiveMetered(manager))
        pushCurrentVpn(manager)
    }

    fun isMetered(): Boolean =
        effectiveMetered(activity.getSystemService(ConnectivityManager::class.java))

    fun isVpnActive(): Boolean =
        isVpnActive(activity.getSystemService(ConnectivityManager::class.java))

    fun networkKey(): String =
        networkPathKey(activity.getSystemService(ConnectivityManager::class.java).activeNetwork)

    /**
     * 一次性返回 VPN 标志与网络路径标识，避免两次 JsBridge 调用之间网络切换
     * 拼出从未存在过的组合。返回 JSON 字符串；旧前端仍可用上面两个方法。
     */
    fun networkSnapshot(): String {
        val manager = activity.getSystemService(ConnectivityManager::class.java)
        val network = manager.activeNetwork
        return JSONObject()
            .put("active", isVpnActive(manager, network))
            .put("metered", effectiveMetered(manager, network))
            .put("networkKey", networkPathKey(network))
            .toString()
    }

    /**
     * 避免异步回调就绪前的冷启动竞态：同步抓取已有的物理网络填入缓存。
     */
    private fun seedPhysicalNetworks(manager: ConnectivityManager) {
        try {
            for (network in manager.allNetworks) {
                val capabilities = manager.getNetworkCapabilities(network) ?: continue
                if (capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
                    capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
                ) {
                    physicalNetworks[network] = PhysicalNetworkInfo(
                        physicalKind(capabilities),
                        capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED),
                        capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED),
                    )
                }
            }
        } catch (_: Exception) {}
    }

    private fun effectiveMetered(
        manager: ConnectivityManager,
        network: Network? = manager.activeNetwork,
    ): Boolean {
        val caps = manager.getNetworkCapabilities(network)
        if (caps != null && caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) {
            // Android 10+ 许多 VPN 默认不声明 NOT_METERED；当流量经 VPN 隧道走时，
            // 依据底层物理网络的计费属性判断，避免连接 Wi-Fi 开 VPN 误触发「仅 Wi-Fi 下载」拦截。
            val best = physicalNetworks.entries.minWithOrNull(
                compareBy<Map.Entry<Network, PhysicalNetworkInfo>>(
                    { if (it.value.validated) 0 else 1 },
                    { physicalRank(it.value.kind) },
                    { it.key.toString() },
                ),
            )
            if (best != null) {
                return !best.value.notMetered
            }
        }
        return manager.isActiveNetworkMetered
    }

    /**
     * 同一时刻采样 VPN 标志与网络路径标识：分开调用会在网络切换瞬间拼出
     * 从未存在过的 (active, networkKey) 组合。
     */
    private fun pushCurrentVpn(manager: ConnectivityManager) {
        val network = manager.activeNetwork
        pushVpn(isVpnActive(manager, network), networkPathKey(network))
    }

    /**
     * 当前默认网络是否由 VPN 承载。
     *
     * 只看默认网络：部分 ROM 会常驻一个未激活的 VPN 配置（或分应用代理），
     * 按 TRANSPORT_VPN 全局查询会把没开 VPN 的用户也判成"已开启"，从而错误地
     * 跳过用户手动配置的代理。默认网络的能力里出现 TRANSPORT_VPN 才说明流量
     * 真的走 VPN。NET_CAPABILITY_NOT_VPN 在 API 28+ 才存在，这里用传输类型判断。
     */
    private fun isVpnActive(
        manager: ConnectivityManager,
        network: Network? = manager.activeNetwork,
    ): Boolean {
        val capabilities = manager.getNetworkCapabilities(network) ?: return false
        return capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN)
    }

    /**
     * 路径标识 = 默认网络 + 当前承载流量的物理网络。
     * VPN 开启时默认网络是隧道，只有后半段能反映底层 Wi-Fi/蜂窝切换。
     */
    private fun networkPathKey(network: Network?): String =
        "${network?.toString() ?: "none"}|${physicalPathKey(network)}"

    private fun updatePhysicalNetwork(manager: ConnectivityManager, network: Network): Boolean {
        val capabilities = manager.getNetworkCapabilities(network) ?: return false
        physicalNetworks[network] = PhysicalNetworkInfo(
            physicalKind(capabilities),
            capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED),
            capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED),
        )
        return true
    }

    private fun physicalKind(capabilities: NetworkCapabilities): String = when {
        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cell"
        else -> "other"
    }

    /**
     * Wi-Fi/有线优先，其次蜂窝；同类型按 netId 排序，保证标识稳定。
     * 未开启 VPN 时直接采用当前默认网络，避免非默认的备用物理网络引发虚假切换；
     * 开启 VPN 时优先挑选已验证（validated）的底层物理链路。
     */
    private fun physicalPathKey(activeNetwork: Network? = null): String {
        val manager = activity.getSystemService(ConnectivityManager::class.java)
        if (activeNetwork != null) {
            val caps = manager.getNetworkCapabilities(activeNetwork)
            if (caps != null && !caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) {
                return "${physicalKind(caps)}:$activeNetwork"
            }
        }
        val best = physicalNetworks.entries.minWithOrNull(
            compareBy<Map.Entry<Network, PhysicalNetworkInfo>>(
                { if (it.value.validated) 0 else 1 },
                { physicalRank(it.value.kind) },
                { it.key.toString() },
            ),
        ) ?: return "none"
        return "${best.value.kind}:${best.key}"
    }

    private fun physicalRank(kind: String): Int = when (kind) {
        "wifi", "ethernet" -> 0
        "cell" -> 1
        else -> 2
    }

    private fun pushMetered(metered: Boolean) {
        activity.runOnUiThread {
            if (lastPushedMetered == metered) return@runOnUiThread
            lastPushedMetered = metered
            try {
                webViewProvider()?.evaluateJavascript(
                    "window.dispatchEvent(new CustomEvent('jm-network-metered', {detail:$metered}))",
                    null,
                )
            } catch (error: Exception) {
                android.util.Log.w("JoyManga", "推送计费状态失败", error)
            }
        }
    }

    /**
     * VPN 与网络路径标识一起推送：后端据此决定是否跳过手动 HTTP 代理
     * （见 Config::effective_proxy_url），并在路径变化时丢弃旧网络的线路健康状态。
     *
     * 不按 `visible` 过滤：下载在后台 Service 侧继续，换网/VPN 变化必须送到 Rust，
     * 否则后台下载会一直沿用旧路径。WebView 已销毁时推送失败只记日志。
     */
    private fun pushVpn(active: Boolean, networkKey: String?) {
        activity.runOnUiThread {
            if (lastPushedVpnActive == active && lastPushedNetworkKey == networkKey) return@runOnUiThread
            lastPushedVpnActive = active
            lastPushedNetworkKey = networkKey
            // 用 JSONObject 生成整个 detail，避免把 Network.toString() 直接拼进脚本。
            val detail = JSONObject().put("active", active).put("networkKey", networkKey ?: JSONObject.NULL)
            try {
                webViewProvider()?.evaluateJavascript(
                    "window.dispatchEvent(new CustomEvent('jm-network-vpn', {detail:$detail}))",
                    null,
                )
            } catch (error: Exception) {
                android.util.Log.w("JoyManga", "推送 VPN 状态失败", error)
            }
        }
    }
}
