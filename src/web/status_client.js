(function () {
  if (window.translateJobStatus) {
    return;
  }

  const LABELS = {
    pending: '待处理',
    processing: '处理中',
    completed: '已完成',
    failed: '已失败',
    queued: '排队中',
  };

  window.translateJobStatus = function (status) {
    if (!status) {
      return '未知';
    }
    const key = String(status).toLowerCase();
    return LABELS[key] || status;
  };
})();
