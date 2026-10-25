// 仅用于 /tests/ui.html：把封面墙对 https://<模拟图床>/media/albums/<id>_3x4.jpg 的请求
// 就地生成为 SVG。应用里图片地址由配置拼出，测试页没有真实图床，不拦截的话整面墙都是占位图，
// 无法核对封面密度、裁切与浮层位置。
// 阅读器书页走 http://jmimg.localhost/<编码后的路径>，同样在这里就地生成，方便核对阅读器界面。
const COLORS = ['#a491bc', '#8aaba6', '#bfa093', '#87a0bd', '#b393a1']

function cover(id) {
  const color = COLORS[Math.abs(id) % COLORS.length]
  return `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="320" viewBox="0 0 240 320">
    <rect width="240" height="320" fill="${color}"/>
    <circle cx="190" cy="65" r="90" fill="white" opacity=".13"/>
    <path d="M-20 235L100 100L270 300H-20Z" fill="white" opacity=".2"/>
    <path d="M-20 290L130 165L270 340H-20Z" fill="#333" opacity=".12"/>
    <text x="22" y="50" fill="white" font-size="12" font-family="sans-serif">ONLINE COVER</text>
    <text x="22" y="252" fill="white" font-size="22" font-family="sans-serif">JM ${id}</text>
  </svg>`
}

// 改名/改规则后立即接管，避免测试页一直用旧版本拦截
self.addEventListener('install', () => self.skipWaiting())
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()))

/** 阅读器书页：竖版单页，标出这是模拟数据 */
function page(path, index) {
  const color = COLORS[index % COLORS.length]
  return `<svg xmlns="http://www.w3.org/2000/svg" width="720" height="1080" viewBox="0 0 720 1080">
    <rect width="720" height="1080" fill="${color}"/>
    <rect x="40" y="40" width="640" height="1000" fill="none" stroke="white" stroke-opacity=".5" stroke-width="3"/>
    <text x="60" y="120" fill="white" font-size="28" font-family="sans-serif">MOCK PAGE ${index + 1}</text>
    <text x="60" y="1010" fill="white" font-size="20" font-family="sans-serif">${path.slice(0, 48)}</text>
  </svg>`
}

self.addEventListener('fetch', (event) => {
  const { pathname } = new URL(event.request.url)
  const coverMatch = pathname.match(/\/media\/albums\/(-?\d+)_3x4\.jpg$/)
  if (coverMatch) {
    event.respondWith(
      new Response(cover(Number(coverMatch[1])), {
        headers: { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'no-store' },
      }),
    )
    return
  }
  const pageMatch = pathname.match(/^\/(fixture-page-(\d+))$/)
  if (pageMatch) {
    event.respondWith(
      new Response(page(pageMatch[1], Number(pageMatch[2]) - 1), {
        headers: { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'no-store' },
      }),
    )
  }
})
