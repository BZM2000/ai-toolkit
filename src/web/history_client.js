(() => {
  const STATUS_LABELS = {
    pending: '排队中',
    processing: '处理中',
    completed: '已完成',
    failed: '失败',
  };
  const POLL_INTERVAL_MS = 20000;
  const panelTimers = new WeakMap();

  document.addEventListener('DOMContentLoaded', () => {
    initTabs();
    initHistoryPanels();
  });

  function initTabs() {
    document.querySelectorAll('[data-tab-group]').forEach((group) => {
      const key = group.dataset.tabGroup;
      if (!key) return;
      const container = document.querySelector(`[data-tab-container="${key}"]`);
      if (!container) return;

      const buttons = group.querySelectorAll('[data-tab-target]');
      buttons.forEach((button) => {
        button.addEventListener('click', () => {
          const target = button.dataset.tabTarget;
          if (!target) return;

          buttons.forEach((btn) => {
            btn.classList.toggle('active', btn === button);
          });

          container.querySelectorAll('[data-tab-panel]').forEach((panel) => {
            panel.classList.toggle('active', panel.dataset.tabPanel === target);
          });
        });
      });
    });
  }

  function initHistoryPanels() {
    document
      .querySelectorAll('.history-panel[data-history-module]')
      .forEach((panel) => {
        const moduleKey = panel.dataset.historyModule;
        if (!moduleKey) return;

        const limitAttr = panel.dataset.historyLimit;
        const limit = limitAttr ? parseInt(limitAttr, 10) || 20 : 20;
        const fetchAndRender = async () => {
          await loadHistory(panel, moduleKey, limit);
        };

        fetchAndRender();
        const timerId = window.setInterval(fetchAndRender, POLL_INTERVAL_MS);
        panelTimers.set(panel, timerId);
      });
  }

  async function loadHistory(panel, moduleKey, limit) {
    const tbody = panel.querySelector('[data-history-body]');
    if (!tbody) {
      return;
    }

    try {
      const response = await fetch(`/api/history?module=${encodeURIComponent(moduleKey)}&limit=${limit}`);
      if (response.status === 401) {
        stopPolling(panel);
        tbody.innerHTML = '<tr class="history-empty-row"><td colspan="4">登录已过期，请刷新页面。</td></tr>';
        return;
      }
      if (!response.ok) {
        throw new Error(`请求失败：${response.status}`);
      }

      const data = await response.json();
      renderHistoryTable(panel, tbody, data.jobs || []);
    } catch (error) {
      console.error('Failed to load history', error);
      tbody.innerHTML = '<tr class="history-empty-row"><td colspan="4">无法加载历史记录，请稍后再试。</td></tr>';
    }
  }

  function stopPolling(panel) {
    const timerId = panelTimers.get(panel);
    if (timerId) {
      window.clearInterval(timerId);
      panelTimers.delete(panel);
    }
  }

  function renderHistoryTable(panel, tbody, jobs) {
    tbody.innerHTML = '';

    if (!jobs.length) {
      const row = document.createElement('tr');
      row.className = 'history-empty-row';
      row.innerHTML = '<td colspan="4">暂无记录。</td>';
      tbody.appendChild(row);
      return;
    }

    jobs.forEach((job) => {
      const { row, detailRow } = buildHistoryRow(job);
      tbody.appendChild(row);
      tbody.appendChild(detailRow);

      const actionButton = row.querySelector('[data-history-action]');
      const detailContainer = detailRow.querySelector('.history-detail');

      if (!actionButton || !detailContainer) {
        return;
      }

      if (job.files_purged) {
        actionButton.disabled = true;
        row.classList.add('history-row-expired');
        return;
      }

      actionButton.addEventListener('click', () => {
        const isActive = detailRow.classList.toggle('active');
        if (!isActive) {
          return;
        }
        if (detailContainer.dataset.loaded === 'true') {
          return;
        }
        loadJobDetail(job, detailContainer);
      });
    });
  }

  function buildHistoryRow(job) {
    const row = document.createElement('tr');
    row.className = 'history-row';

    const statusLabel = translateStatus(job.status);
    const updatedLabel = formatDateTime(job.updated_at);
    const createdLabel = formatDateTime(job.created_at);

    row.innerHTML = `
      <td>
        <div class="history-job-title">
          <span class="job-name">${escapeHtml(job.module_label || '任务')}</span>
          <span class="job-meta">ID: ${escapeHtml(job.job_key)}</span>
          <span class="job-meta">提交时间：${escapeHtml(createdLabel)}</span>
          ${job.files_purged ? '<span class="job-warning">结果已自动清除</span>' : ''}
        </div>
      </td>
      <td>
        <span class="status-badge status-${escapeHtml(job.status || 'unknown')}">${escapeHtml(statusLabel)}</span>
        ${job.files_purged ? '<span class="status-badge status-expired">已清理</span>' : ''}
        ${job.status_detail ? `<div class="history-status-detail">${escapeHtml(job.status_detail)}</div>` : ''}
      </td>
      <td>${escapeHtml(updatedLabel)}</td>
      <td class="history-actions">
        <button type="button" data-history-action>查看详情</button>
      </td>
    `;

    const detailRow = document.createElement('tr');
    detailRow.className = 'history-detail-row';
    detailRow.innerHTML = `
      <td colspan="4">
        <div class="history-detail" data-status-url="${escapeAttribute(job.status_path)}" data-module="${escapeAttribute(job.module)}">
          <p class="history-status-detail">正在加载详情...</p>
        </div>
      </td>
    `;

    if (job.files_purged) {
      const detail = detailRow.querySelector('.history-detail');
      if (detail) {
        detail.dataset.loaded = 'true';
        detail.innerHTML = '<p class="history-status-detail">该任务的输出已过期并被清除。</p>';
      }
    }

    return { row, detailRow };
  }

  async function loadJobDetail(job, container) {
    const statusUrl = container.dataset.statusUrl;
    const moduleKey = container.dataset.module;
    if (!statusUrl) {
      container.innerHTML = '<p class="history-status-detail">无法加载详情。</p>';
      container.dataset.loaded = 'true';
      return;
    }

    try {
      const response = await fetch(statusUrl);
      if (!response.ok) {
        throw new Error(`状态请求失败：${response.status}`);
      }
      const data = await response.json();
      container.dataset.loaded = 'true';
      renderJobDetail(container, moduleKey, job, data);
    } catch (error) {
      console.error('Failed to load job detail', error);
      container.dataset.loaded = 'true';
      container.innerHTML = '<p class="history-status-detail">无法加载详情，请稍后再试。</p>';
    }
  }

  function renderJobDetail(container, moduleKey, job, status) {
    const fragments = [];

    const summaryParts = [];
    if (status.status) {
      summaryParts.push(`<strong>状态：</strong>${escapeHtml(translateStatus(status.status))}`);
    }
    if (status.status_detail) {
      summaryParts.push(`<strong>说明：</strong>${escapeHtml(status.status_detail)}`);
    }
    if (status.error_message || status.error) {
      summaryParts.push(`<strong>错误：</strong>${escapeHtml(status.error_message || status.error)}`);
    }

    fragments.push(`<div class="history-summary">${summaryParts.join('<br>')}</div>`);

    const downloads = renderDownloads(moduleKey, status);
    if (downloads) {
      fragments.push(downloads);
    }

    const extra = renderExtraInfo(moduleKey, status);
    if (extra) {
      fragments.push(extra);
    }

    container.innerHTML = fragments.join('');
  }

  function renderDownloads(moduleKey, status) {
    const links = [];

    if (moduleKey === 'summarizer') {
      if (status.combined_summary_url) {
        links.push(createDownloadLink('汇总摘要', status.combined_summary_url));
      }
      if (status.combined_translation_url) {
        links.push(createDownloadLink('汇总译文', status.combined_translation_url));
      }
      if (Array.isArray(status.documents)) {
        status.documents.forEach((doc, index) => {
          if (doc.summary_download_url) {
            links.push(createDownloadLink(`文档 ${index + 1} 摘要`, doc.summary_download_url));
          }
          if (doc.translation_download_url) {
            links.push(createDownloadLink(`文档 ${index + 1} 译文`, doc.translation_download_url));
          }
        });
      }
    } else if (moduleKey === 'translatedocx') {
      if (Array.isArray(status.documents)) {
        status.documents.forEach((doc, index) => {
          if (doc.translated_download_url) {
            links.push(createDownloadLink(`译文 ${index + 1}`, doc.translated_download_url));
          }
        });
      }
    } else if (moduleKey === 'info_extract') {
      if (status.result_download_url) {
        links.push(createDownloadLink('下载结果表', status.result_download_url));
      }
    } else if (moduleKey === 'reviewer') {
      if (Array.isArray(status.round1_reviews)) {
        status.round1_reviews.forEach((review, idx) => {
          if (review.download_url) {
            links.push(createDownloadLink(`第一轮审稿 ${idx + 1}`, review.download_url));
          }
        });
      }
      if (status.round2_review && status.round2_review.download_url) {
        links.push(createDownloadLink('第二轮元审稿', status.round2_review.download_url));
      }
      if (status.round3_review && status.round3_review.download_url) {
        links.push(createDownloadLink('第三轮事实核查', status.round3_review.download_url));
      }
    }

    if (!links.length) {
      return '';
    }

    return `<div class="history-downloads">${links.join('')}</div>`;
  }

  function renderExtraInfo(moduleKey, status) {
    if (moduleKey === 'grader') {
      const parts = [];
      if (typeof status.iqm_score === 'number') {
        parts.push(`<strong>评分：</strong>${status.iqm_score.toFixed(2)}`);
      }
      if (status.justification) {
        parts.push(`<strong>理由：</strong>${escapeHtml(status.justification)}`);
      }
      if (status.decision_reason) {
        parts.push(`<strong>结论：</strong>${escapeHtml(status.decision_reason)}`);
      }
      if (status.keyword_main || (Array.isArray(status.keyword_peripherals) && status.keyword_peripherals.length)) {
        const extras = [];
        if (status.keyword_main) {
          extras.push(`主题：${escapeHtml(status.keyword_main)}`);
        }
        if (Array.isArray(status.keyword_peripherals) && status.keyword_peripherals.length) {
          extras.push(`关联关键词：${escapeHtml(status.keyword_peripherals.join('、'))}`);
        }
        parts.push(`<strong>关键词：</strong>${extras.join('；')}`);
      }
      if (status.recommendations && Array.isArray(status.recommendations) && status.recommendations.length) {
        const items = status.recommendations
          .map((rec) => {
            const score = typeof rec.match_score === 'number' ? `匹配度 ${(rec.match_score * 100).toFixed(0)}%` : '';
            const ref = rec.reference_mark ? `（${escapeHtml(rec.reference_mark)}）` : '';
            return `<li>${escapeHtml(rec.journal_name || '期刊')}${ref}${score ? ` - ${score}` : ''}</li>`;
          })
          .join('');
        parts.push(`<strong>推荐期刊：</strong><ul>${items}</ul>`);
      }
      if (!parts.length) {
        return '';
      }
      return `<div class="history-extra">${parts.join('<br>')}</div>`;
    }

    if (moduleKey === 'summarizer' && Array.isArray(status.documents)) {
      const failedDocs = status.documents.filter((doc) => doc.error_message);
      if (failedDocs.length) {
        const items = failedDocs
          .map((doc, idx) => `<li>文档 ${idx + 1}：${escapeHtml(doc.error_message)}</li>`)
          .join('');
        return `<div class="history-extra"><strong>失败文档：</strong><ul>${items}</ul></div>`;
      }
    }

    return '';
  }

  function createDownloadLink(label, href) {
    return `<a href="${escapeAttribute(href)}" target="_blank" rel="noopener">${escapeHtml(label)} 下载</a>`;
  }

  function translateStatus(status) {
    if (!status) return '未知';
    const lowered = status.toLowerCase();
    return STATUS_LABELS[lowered] || status;
  }

  function formatDateTime(isoString) {
    if (!isoString) return '—';
    const date = new Date(isoString);
    if (Number.isNaN(date.getTime())) {
      return isoString;
    }
    return date.toLocaleString('zh-CN', { hour12: false });
  }

  function escapeHtml(value) {
    if (value === undefined || value === null) {
      return '';
    }
    return String(value)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function escapeAttribute(value) {
    return escapeHtml(value).replace(/`/g, '&#96;');
  }
})();
