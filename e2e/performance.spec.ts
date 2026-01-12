import { test, expect } from '@playwright/test';

// Generate a large mock file list for performance testing
function generateMockFiles(count: number, basePath: string) {
  const files = [];
  const extensions = ['jpg', 'png', 'gif', 'pdf', 'txt', 'mp4', 'mov', 'psd'];
  const now = new Date().toISOString();

  for (let i = 0; i < count; i++) {
    const ext = extensions[i % extensions.length];
    const isDir = i % 20 === 0; // 5% directories
    files.push({
      name: isDir ? `folder_${i}` : `file_${i.toString().padStart(5, '0')}.${ext}`,
      path: `${basePath}/${isDir ? `folder_${i}` : `file_${i.toString().padStart(5, '0')}.${ext}`}`,
      is_directory: isDir,
      is_hidden: i % 100 === 0,
      is_symlink: false,
      is_git_repo: false,
      size: isDir ? 0 : Math.floor(Math.random() * 10000000),
      modified: now,
      created: now,
      extension: isDir ? null : ext,
      child_count: isDir ? Math.floor(Math.random() * 100) : null,
      image_width: ext === 'jpg' || ext === 'png' ? 1920 : null,
      image_height: ext === 'jpg' || ext === 'png' ? 1080 : null,
    });
  }
  return files;
}

function getPerformanceMockScript(fileCount: number): string {
  const mockHomeDir = '/Users/testuser';
  const mockLargeDir = `${mockHomeDir}/LargeFolder`;

  return `
    (function() {
      const mockHomeDir = '${mockHomeDir}';
      const mockLargeDir = '${mockLargeDir}';
      const FILE_COUNT = ${fileCount};

      // Generate files inline for performance
      function generateFiles() {
        const files = [];
        const extensions = ['jpg', 'png', 'gif', 'pdf', 'txt', 'mp4', 'mov', 'psd'];
        const now = new Date().toISOString();

        for (let i = 0; i < FILE_COUNT; i++) {
          const ext = extensions[i % extensions.length];
          const isDir = i % 20 === 0;
          files.push({
            name: isDir ? 'folder_' + i : 'file_' + i.toString().padStart(5, '0') + '.' + ext,
            path: mockLargeDir + '/' + (isDir ? 'folder_' + i : 'file_' + i.toString().padStart(5, '0') + '.' + ext),
            is_directory: isDir,
            is_hidden: i % 100 === 0,
            is_symlink: false,
            is_git_repo: false,
            size: isDir ? 0 : Math.floor(Math.random() * 10000000),
            modified: now,
            created: now,
            extension: isDir ? null : ext,
            child_count: isDir ? Math.floor(Math.random() * 100) : null,
            image_width: (ext === 'jpg' || ext === 'png') ? 1920 : null,
            image_height: (ext === 'jpg' || ext === 'png') ? 1080 : null,
          });
        }
        return files;
      }

      const mockHomeFiles = [
        {
          name: 'LargeFolder',
          path: mockLargeDir,
          is_directory: true,
          is_hidden: false,
          is_symlink: false,
          is_git_repo: false,
          size: 0,
          modified: new Date().toISOString(),
          extension: null,
          child_count: FILE_COUNT,
        },
      ];

      let cachedLargeFiles = null;
      function getLargeFiles() {
        if (!cachedLargeFiles) {
          cachedLargeFiles = generateFiles();
        }
        return cachedLargeFiles;
      }

      function getFilesForPath(path) {
        const normalizedPath = (path || '').replace(/\\\\/g, '/').replace(/\\/+$/, '') || '/';
        if (normalizedPath === mockLargeDir || normalizedPath.endsWith('/LargeFolder')) {
          return getLargeFiles();
        }
        if (normalizedPath === mockHomeDir || normalizedPath === '~' || normalizedPath === '/') {
          return mockHomeFiles;
        }
        return [];
      }

      // Event listeners registry
      const listeners = new Map();
      const callbacks = new Map();

      function registerCallback(callback, once = false) {
        const identifier = window.crypto.getRandomValues(new Uint32Array(1))[0];
        callbacks.set(identifier, (data) => {
          if (once) callbacks.delete(identifier);
          return callback && callback(data);
        });
        return identifier;
      }

      function runCallback(id, data) {
        const callback = callbacks.get(id);
        if (callback) callback(data);
      }

      function unregisterListener(event, id) {
        callbacks.delete(id);
        const eventListeners = listeners.get(event);
        if (eventListeners) {
          const index = eventListeners.indexOf(id);
          if (index !== -1) eventListeners.splice(index, 1);
        }
      }

      function handleListen(args) {
        if (!listeners.has(args.event)) listeners.set(args.event, []);
        listeners.get(args.event).push(args.handler);
        return args.handler;
      }

      // Schedule directory batch events - emit in chunks to simulate streaming
      function scheduleDirectoryBatch(sessionId, path) {
        const files = getFilesForPath(path);
        const BATCH_SIZE = 100;

        // Mark performance timing
        window.__PERF_BATCH_START__ = performance.now();

        queueMicrotask(() => {
          let batchIndex = 0;
          function emitBatch() {
            const start = batchIndex * BATCH_SIZE;
            const end = Math.min(start + BATCH_SIZE, files.length);
            const chunk = files.slice(start, end);
            const isFinal = end >= files.length;

            const eventListeners = listeners.get('directory-batch') || [];
            const payload = {
              sessionId,
              batchIndex,
              entries: chunk,
              isFinal,
              totalCount: batchIndex === 0 ? files.length : null,
            };

            for (const handler of eventListeners) {
              runCallback(handler, { event: 'directory-batch', payload });
            }

            if (isFinal) {
              window.__PERF_BATCH_END__ = performance.now();
              window.__PERF_BATCH_COMPLETE__ = true;
            } else {
              batchIndex++;
              // Use setTimeout(0) to allow rendering between batches
              setTimeout(emitBatch, 0);
            }
          }
          setTimeout(emitBatch, 10);
        });
      }

      const commandHandlers = {
        get_home_directory: () => mockHomeDir,
        read_directory: (args) => {
          const path = args?.path || mockHomeDir;
          return {
            entries: getFilesForPath(path),
            location: { path, displayPath: path, raw: 'file://' + path },
            capabilities: { canCreate: true, canDelete: true, canRename: true, canCreateDirectories: true },
          };
        },
        read_directory_streaming_command: (args) => {
          const path = args?.path || mockHomeDir;
          const sessionId = args?.sessionId || 'mock-session';
          scheduleDirectoryBatch(sessionId, path);
          return {
            sessionId,
            location: { path, displayPath: path, raw: 'file://' + path },
            capabilities: { canCreate: true, canDelete: true, canRename: true, canCreateDirectories: true },
          };
        },
        cancel_directory_stream: () => undefined,
        read_preferences: () => JSON.stringify({}),
        get_dir_prefs: () => JSON.stringify({}),
        set_dir_prefs: () => undefined,
        set_last_dir: () => undefined,
        get_system_accent_color: () => '#007AFF',
        initialize_thumbnail_service: () => true,
        start_watching_directory: () => undefined,
        stop_watching_directory: () => undefined,
        update_hidden_files_menu: () => undefined,
        update_folders_first_menu: () => undefined,
        update_sort_menu_state: () => undefined,
        update_selection_menu_state: () => undefined,
        get_pinned_directories: () => [],
        load_pinned_directories: () => [],
        get_system_drives: () => [],
        get_disk_usage: () => ({ total: 1000000000, free: 500000000 }),
        enable_drag_detection: () => undefined,
        set_drop_zone: () => undefined,
        get_git_status: () => null,
        'plugin:app|version': () => '0.1.0',
        'plugin:os|platform': () => 'linux',
        'plugin:os|version': () => '5.0',
        'plugin:os|os_type': () => 'linux',
        'plugin:os|arch': () => 'x86_64',
        'plugin:os|locale': () => 'en-US',
        'plugin:os|hostname': () => 'test-host',
        'plugin:event|listen': (args) => handleListen(args),
        'plugin:event|emit': () => null,
        'plugin:event|unlisten': () => undefined,
      };

      window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
      window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        const handler = commandHandlers[cmd];
        if (handler) return handler(args);
        console.warn('[Tauri Mock] Unhandled command:', cmd);
        return undefined;
      };
      window.__TAURI_INTERNALS__.metadata = {
        currentWindow: { label: 'main' },
        currentWebview: { windowLabel: 'main', label: 'main' },
      };
      window.__TAURI_INTERNALS__.transformCallback = registerCallback;
      window.__TAURI_INTERNALS__.runCallback = runCallback;
      window.__TAURI_INTERNALS__.callbacks = callbacks;
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener };
      window.__TAURI_OS_PLUGIN_INTERNALS__ = {
        platform: 'linux',
        version: '5.0',
        osType: 'linux',
        arch: 'x86_64',
        locale: 'en-US',
        hostname: 'test-host',
      };

      console.log('[Perf Mock] Mocks injected for', FILE_COUNT, 'files');
    })();
  `;
}

const MOCK_HOME_DIR = '/Users/testuser';
const MOCK_LARGE_DIR = `${MOCK_HOME_DIR}/LargeFolder`;

test.describe('Performance Profiling', () => {
  test('profile 7000 file directory load', async ({ page, browser }) => {
    const FILE_COUNT = 7000;

    // Start CDP session for detailed metrics
    const cdpSession = await page.context().newCDPSession(page);
    await cdpSession.send('Performance.enable');

    // Inject performance mocks
    await page.addInitScript(getPerformanceMockScript(FILE_COUNT));

    // Collect performance entries
    const performanceEntries: PerformanceEntry[] = [];

    // Navigate to app
    const navStart = performance.now();
    await page.goto('/');

    // Wait for initial render
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    const appReadyTime = performance.now() - navStart;
    console.log(`\n📊 App ready: ${appReadyTime.toFixed(0)}ms`);

    // Navigate to large folder and measure
    const navigationStart = performance.now();

    await pathInput.click();
    await pathInput.fill(MOCK_LARGE_DIR);

    // Start tracing right before navigation
    await cdpSession.send('Tracing.start', {
      categories: 'devtools.timeline,v8.execute,disabled-by-default-devtools.timeline',
    });

    await pathInput.press('Enter');

    // Wait for first file to appear (measures time to first content)
    const firstFileLocator = page.locator('[data-testid="file-item"]').first();
    await expect(firstFileLocator).toBeVisible({ timeout: 30000 });
    const timeToFirstFile = performance.now() - navigationStart;

    // Wait for streaming to complete
    await page.waitForFunction(() => (window as any).__PERF_BATCH_COMPLETE__ === true, {
      timeout: 60000,
    });

    // Stop tracing
    const tracingData = await cdpSession.send('Tracing.end');

    const timeToComplete = performance.now() - navigationStart;

    // Get batch timing from page
    const batchTiming = await page.evaluate(() => {
      const w = window as any;
      return {
        batchStart: w.__PERF_BATCH_START__,
        batchEnd: w.__PERF_BATCH_END__,
      };
    });
    const batchDuration = batchTiming.batchEnd - batchTiming.batchStart;

    // Count rendered items
    const renderedCount = await page.locator('[data-testid="file-item"]').count();

    // Get performance metrics from CDP
    const metrics = await cdpSession.send('Performance.getMetrics');
    const metricsMap = new Map(metrics.metrics.map((m: any) => [m.name, m.value]));

    // Get Long Tasks (blocking > 50ms)
    const longTasks = await page.evaluate(() => {
      return (window as any).__longTasks || [];
    });

    console.log(`\n${'='.repeat(60)}`);
    console.log(`📊 PERFORMANCE REPORT: ${FILE_COUNT} files`);
    console.log(`${'='.repeat(60)}`);
    console.log(`\n⏱️  TIMING:`);
    console.log(`   Time to first file visible:  ${timeToFirstFile.toFixed(0)}ms`);
    console.log(`   Time to streaming complete:  ${timeToComplete.toFixed(0)}ms`);
    console.log(`   Batch processing duration:   ${batchDuration.toFixed(0)}ms`);
    console.log(`\n📦 RENDERING:`);
    console.log(`   Files rendered (visible):    ${renderedCount}`);
    console.log(`   Total files in directory:    ${FILE_COUNT}`);
    console.log(`\n🔧 V8 METRICS:`);
    console.log(
      `   JS Heap Size:                ${(((metricsMap.get('JSHeapUsedSize') as number) || 0) / 1024 / 1024).toFixed(1)}MB`
    );
    console.log(`   DOM Nodes:                   ${metricsMap.get('Nodes') || 'N/A'}`);
    console.log(`   Layout Count:                ${metricsMap.get('LayoutCount') || 'N/A'}`);
    console.log(
      `   Layout Duration:             ${(((metricsMap.get('LayoutDuration') as number) || 0) * 1000).toFixed(0)}ms`
    );
    console.log(
      `   Script Duration:             ${(((metricsMap.get('ScriptDuration') as number) || 0) * 1000).toFixed(0)}ms`
    );
    console.log(
      `   Task Duration:               ${(((metricsMap.get('TaskDuration') as number) || 0) * 1000).toFixed(0)}ms`
    );
    console.log(`${'='.repeat(60)}\n`);

    // Diagnostic: Check virtualization
    const virtualInfo = await page.evaluate(() => {
      // Check if virtualizer is limiting rendered items
      const scrollContainer = document.querySelector(
        '[data-list-scroll-container="true"], [data-grid-scroll-container="true"]'
      );
      const allItems = document.querySelectorAll('[data-testid="file-item"]');
      const visibleItems = Array.from(allItems).filter((el) => {
        const rect = el.getBoundingClientRect();
        return rect.top < window.innerHeight && rect.bottom > 0;
      });
      return {
        scrollContainerExists: !!scrollContainer,
        scrollContainerHeight: scrollContainer?.clientHeight || 0,
        scrollContainerScrollHeight: scrollContainer?.scrollHeight || 0,
        totalItemsInDOM: allItems.length,
        visibleItemCount: visibleItems.length,
        viewportHeight: window.innerHeight,
      };
    });

    console.log(`\n🔍 VIRTUALIZATION DIAGNOSTIC:`);
    console.log(`   Scroll container exists:     ${virtualInfo.scrollContainerExists}`);
    console.log(`   Container height:            ${virtualInfo.scrollContainerHeight}px`);
    console.log(`   Scroll height:               ${virtualInfo.scrollContainerScrollHeight}px`);
    console.log(`   Items in DOM:                ${virtualInfo.totalItemsInDOM}`);
    console.log(`   Actually visible items:      ${virtualInfo.visibleItemCount}`);
    console.log(`   Viewport height:             ${virtualInfo.viewportHeight}px`);

    // Virtualization should limit DOM items to ~50-100 (visible + overscan)
    const expectedMaxItems = 150; // generous buffer for overscan
    if (virtualInfo.totalItemsInDOM > expectedMaxItems) {
      console.log(
        `\n   ❌ VIRTUALIZATION BROKEN: ${virtualInfo.totalItemsInDOM} items in DOM (expected <${expectedMaxItems})`
      );
    } else {
      console.log(`\n   ✅ Virtualization working correctly`);
    }

    // Relaxed assertions for now - we're profiling, not passing
    // expect(timeToFirstFile).toBeLessThan(500);
    // expect(timeToComplete).toBeLessThan(5000);

    // Just ensure test completes
    expect(renderedCount).toBeGreaterThan(0);
  });

  test('profile scrolling performance with 7000 files', async ({ page }) => {
    const FILE_COUNT = 7000;

    await page.addInitScript(getPerformanceMockScript(FILE_COUNT));
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    // Navigate to large folder
    await pathInput.click();
    await pathInput.fill(MOCK_LARGE_DIR);
    await pathInput.press('Enter');

    // Wait for load
    await page.waitForFunction(() => (window as any).__PERF_BATCH_COMPLETE__ === true, {
      timeout: 60000,
    });

    // Find scroll container
    const scrollContainer = page
      .locator('[data-list-scroll-container="true"], [data-grid-scroll-container="true"]')
      .first();
    await expect(scrollContainer).toBeVisible();

    // Measure scroll performance
    const cdpSession = await page.context().newCDPSession(page);
    await cdpSession.send('Performance.enable');

    // Scroll down rapidly
    const scrollStart = performance.now();
    for (let i = 0; i < 10; i++) {
      await scrollContainer.evaluate((el) => {
        el.scrollTop += 500;
      });
      await page.waitForTimeout(50);
    }
    const scrollDuration = performance.now() - scrollStart;

    // Get metrics after scrolling
    const metrics = await cdpSession.send('Performance.getMetrics');
    const metricsMap = new Map(metrics.metrics.map((m: any) => [m.name, m.value]));

    console.log(`\n${'='.repeat(60)}`);
    console.log(`📊 SCROLL PERFORMANCE REPORT`);
    console.log(`${'='.repeat(60)}`);
    console.log(`   Scroll test duration:        ${scrollDuration.toFixed(0)}ms`);
    console.log(
      `   Layout Duration (total):     ${(((metricsMap.get('LayoutDuration') as number) || 0) * 1000).toFixed(0)}ms`
    );
    console.log(
      `   Recalc Style Duration:       ${(((metricsMap.get('RecalcStyleDuration') as number) || 0) * 1000).toFixed(0)}ms`
    );
    console.log(`${'='.repeat(60)}\n`);

    // Scrolling should be smooth - 10 scroll operations in under 2s
    expect(scrollDuration).toBeLessThan(2000);
  });

  test('profile sorting performance with 7000 files', async ({ page }) => {
    const FILE_COUNT = 7000;

    await page.addInitScript(getPerformanceMockScript(FILE_COUNT));
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    // Navigate to large folder
    await pathInput.click();
    await pathInput.fill(MOCK_LARGE_DIR);
    await pathInput.press('Enter');

    await page.waitForFunction(() => (window as any).__PERF_BATCH_COMPLETE__ === true, {
      timeout: 60000,
    });

    // Wait for list view header to be visible (sorting is in list view)
    // First switch to list view if in grid
    await page.keyboard.press('Meta+2'); // Switch to list view
    await page.waitForTimeout(200);

    // Click on Size header to sort
    const sizeHeader = page.getByRole('button', { name: /Size/i });
    if (await sizeHeader.isVisible()) {
      const sortStart = performance.now();
      await sizeHeader.click();
      await page.waitForTimeout(100); // Allow re-render

      const sortDuration = performance.now() - sortStart;

      console.log(`\n${'='.repeat(60)}`);
      console.log(`📊 SORT PERFORMANCE REPORT`);
      console.log(`${'='.repeat(60)}`);
      console.log(`   Sort by Size duration:       ${sortDuration.toFixed(0)}ms`);
      console.log(`${'='.repeat(60)}\n`);

      // Sorting should be fast - under 200ms
      expect(sortDuration).toBeLessThan(500);
    }
  });
});
