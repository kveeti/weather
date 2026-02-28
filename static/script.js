const VAPID_PUBLIC_KEY = '{{VAPID_PUBLIC_KEY}}';

function urlBase64ToUint8Array(base64String) {
  const padding = '='.repeat((4 - base64String.length % 4) % 4);
  const base64 = (base64String + padding).replace(/-/g, '+').replace(/_/g, '/');
  const raw = atob(base64);
  return Uint8Array.from(raw, c => c.charCodeAt(0));
}

async function subscribePush() {
  const status = document.getElementById('push-status');
  if (!('serviceWorker' in navigator) || !('PushManager' in window)) {
    status.textContent = 'Push notifications not supported in this browser.';
    return;
  }
  try {
    await navigator.serviceWorker.register('/sw.js');
    const reg = await navigator.serviceWorker.ready;
    const perm = await Notification.requestPermission();
    if (perm !== 'granted') {
      status.textContent = 'Notification permission denied.';
      return;
    }
    const existing = await reg.pushManager.getSubscription();
    if (existing) await existing.unsubscribe();
    const sub = await reg.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: urlBase64ToUint8Array(VAPID_PUBLIC_KEY),
    });
    const json = sub.toJSON();
    const resp = await fetch('/push/subscribe', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        endpoint: json.endpoint,
        p256dh: json.keys.p256dh,
        auth: json.keys.auth,
      }),
    });
    if (resp.ok) {
      status.textContent = 'Push notifications enabled!';
      document.getElementById('push-btn').textContent = 'Disable Push Notifications';
      document.getElementById('push-btn').onclick = unsubscribePush;
    } else {
      status.textContent = 'Failed to register subscription on server.';
    }
  } catch (e) {
    status.textContent = 'Error: ' + e.message;
  }
}

async function unsubscribePush() {
  const status = document.getElementById('push-status');
  try {
    const reg = await navigator.serviceWorker.getRegistration('/sw.js');
    if (!reg) { { status.textContent = 'No service worker found.'; return; } }
    const sub = await reg.pushManager.getSubscription();
    if (!sub) { { status.textContent = 'Not subscribed.'; return; } }
    const endpoint = sub.endpoint;
    await sub.unsubscribe();
    await fetch('/push/unsubscribe', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ endpoint }),
    });
    status.textContent = 'Unsubscribed.';
    document.getElementById('push-btn').textContent = 'Enable Push Notifications';
    document.getElementById('push-btn').onclick = subscribePush;
  } catch (e) {
    status.textContent = 'Error: ' + e.message;
  }
}

(async () => {
  if (!('serviceWorker' in navigator)) return;
  try {
    const reg = await navigator.serviceWorker.getRegistration('/sw.js');
    if (!reg) return;
    const sub = await reg.pushManager.getSubscription();
    if (sub) {
      document.getElementById('push-btn').textContent = 'Disable Push Notifications';
      document.getElementById('push-btn').onclick = unsubscribePush;
    }
  } catch (_) { }
})();

async function testSummary(btn) {
  btn.disabled = true;
  btn.textContent = 'Sending...';
  try {
    const resp = await fetch('/push/test-summary', { method: 'POST' });
    const data = await resp.json();
    btn.textContent = data.ok ? 'Sent!' : 'Failed';
  } catch (e) {
    btn.textContent = 'Error';
  }
  setTimeout(() => { btn.textContent = 'Test Daily Notif'; btn.disabled = false; }, 2000);
}
