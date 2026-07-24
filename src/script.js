// 悬浮文本框 - 前端逻辑
// 通过 window.__TAURI__ 调用 Rust 后端命令

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const appWindow = getCurrentWindow();

// === DOM 引用 ===
const container = document.getElementById('container');
const textEdit = document.getElementById('textEdit');
const countLabel = document.getElementById('countLabel');
const opacitySlider = document.getElementById('opacitySlider');
const pinBtn = document.getElementById('pinBtn');
const minBtn = document.getElementById('minBtn');
const closeBtn = document.getElementById('closeBtn');
const clearBtn = document.getElementById('clearBtn');
const sizeGrip = document.getElementById('sizeGrip');
const toast = document.getElementById('toast');

// === 字符计数 ===
function updateCount() {
  countLabel.textContent = `字符: ${textEdit.value.length}`;
}
textEdit.addEventListener('input', updateCount);

// === 透明度 (用 CSS opacity 模拟窗口透明度) ===
function applyOpacity(value) {
  container.style.opacity = value / 100;
}

opacitySlider.addEventListener('input', () => {
  applyOpacity(opacitySlider.value);
});

// === Shift + 滚轮调透明度 ===
window.addEventListener('wheel', (e) => {
  if (e.shiftKey) {
    e.preventDefault();
    const delta = e.deltaY > 0 ? -1 : 1;
    let newVal = parseInt(opacitySlider.value, 10) + delta;
    newVal = Math.max(30, Math.min(100, newVal));
    opacitySlider.value = newVal;
    applyOpacity(newVal);
  }
}, { passive: false });

// === 置顶切换 ===
async function toggleTopmost() {
  const pinned = await invoke('toggle_topmost');
  pinBtn.dataset.pinned = pinned ? 'true' : 'false';
}
pinBtn.addEventListener('click', toggleTopmost);

// === 最小化 ===
minBtn.addEventListener('click', async () => {
  await appWindow.minimize();
});

// === 关闭到托盘 ===
closeBtn.addEventListener('click', async () => {
  await appWindow.hide();
});

// === 清空文本 ===
function clearText() {
  textEdit.value = '';
  updateCount();
  textEdit.focus();
}
clearBtn.addEventListener('click', clearText);

// === size grip 拖动调整大小 ===
sizeGrip.addEventListener('mousedown', async (e) => {
  e.preventDefault();
  await appWindow.startResizeDragging('SouthEast');
});

// === 应用级快捷键 (窗口聚焦时生效) ===
document.addEventListener('keydown', (e) => {
  if (!e.ctrlKey) return;
  const key = e.key.toLowerCase();
  if (key === 'q') {
    e.preventDefault();
    invoke('quit_app');
  } else if (key === 'l') {
    e.preventDefault();
    clearText();
  } else if (key === 't') {
    e.preventDefault();
    toggleTopmost();
  }
});

// === 监听后端事件 ===
// 清空文本（来自托盘菜单或其他途径）
listen('clear-text', () => clearText());

// 置顶状态变更（来自托盘菜单）
listen('topmost-changed', (event) => {
  const pinned = event.payload;
  pinBtn.dataset.pinned = pinned ? 'true' : 'false';
});

// 隐藏到托盘提示
listen('hidden-to-tray', () => {
  showToast('窗口已最小化到托盘，单击托盘图标可恢复');
});

// === Toast 提示 ===
let toastTimer = null;
function showToast(message) {
  toast.textContent = message;
  toast.classList.add('show');
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toast.classList.remove('show');
  }, 2000);
}

// === 初始化 ===
applyOpacity(opacitySlider.value);
updateCount();
textEdit.focus();
