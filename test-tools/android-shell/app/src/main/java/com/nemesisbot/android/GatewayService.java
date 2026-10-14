package com.nemesisbot.android;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.util.Log;

import androidx.annotation.Nullable;
import androidx.core.app.NotificationCompat;

import java.io.BufferedReader;
import java.io.File;
import java.io.InputStreamReader;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Foreground service that manages the nemesisbot gateway process.
 * Captures stdout/stderr and broadcasts log lines to the UI.
 */
public class GatewayService extends Service {
    private static final String TAG = "GatewayService";
    private static final String CHANNEL_ID = "nemesisbot_service";
    private static final int NOTIFICATION_ID = 1;

    // Broadcast action for log messages
    public static final String ACTION_LOG = "com.nemesisbot.android.LOG";
    public static final String ACTION_STATUS = "com.nemesisbot.android.STATUS";
    public static final String EXTRA_LOG_LINE = "log_line";
    public static final String EXTRA_STATUS = "status";

    // Status constants
    public static final String STATUS_STARTING = "starting";
    public static final String STATUS_RUNNING = "running";
    public static final String STATUS_STOPPED = "stopped";
    public static final String STATUS_ERROR = "error";

    // S4（2026-10-14 审计）：volatile + start/stop 共用 gatewayLock——进程
    // 字段跨线程读写（onStartCommand 主线程 / gateway worker / onDestroy 主
    // 线程），裸字段加「start 后才 set(true)」的窗口曾致双起进程（字段覆盖
    // 出孤儿）与 stop 检查窗口跳过收尸（见 startGateway/stopGateway 注释）。
    private volatile Process gatewayProcess;
    private final Object gatewayLock = new Object();
    private final AtomicBoolean isRunning = new AtomicBoolean(false);
    private BinaryManager binaryManager;
    private Handler mainHandler;
    // Cluster UDP discovery: WiFi drivers filter broadcast/multicast packets by
    // default; holding a MulticastLock for the service lifetime lets the
    // gateway process RECEIVE discovery announces on the WiFi interface.
    private WifiManager.MulticastLock multicastLock;

    // Singleton reference for activity to check status
    private static GatewayService instance;

    public static GatewayService getInstance() {
        return instance;
    }

    public boolean isGatewayRunning() {
        return isRunning.get();
    }

    @Override
    public void onCreate() {
        super.onCreate();
        instance = this;
        binaryManager = new BinaryManager(this);
        mainHandler = new Handler(Looper.getMainLooper());
        createNotificationChannel();
        acquireMulticastLock();
        Log.i(TAG, "GatewayService created");
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        Notification notification = buildNotification("NemesisBot 正在启动...");
        startForeground(NOTIFICATION_ID, notification);

        startGateway();
        // START_STICKY: system restarts the service after a low-memory kill
        // so the bot survives in background (null intent on restart —
        // startGateway() runs regardless of intent contents).
        return START_STICKY;
    }

    @Override
    public void onDestroy() {
        stopGateway();
        releaseMulticastLock();
        instance = null;
        super.onDestroy();
    }

    @Nullable
    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    /**
     * Start the nemesisbot gateway process.
     */
    private void startGateway() {
        // S4（2026-10-14 审计）：与 stopGateway 共用 gatewayLock——快速双击/
        // 重复 onStartCommand 时两个线程都过 isRunning 检查会双起进程（字段
        // 被覆盖后只剩最后一个可杀 → 孤儿）。锁内检查并抢占：先置 running 再
        // 起进程（失败回滚），onDestroy 落在任何窗口 stop 都能看到 running 态。
        synchronized (gatewayLock) {
            if (isRunning.get()) {
                Log.w(TAG, "Gateway already running");
                return;
            }
            isRunning.set(true);
        }

        broadcastStatus(STATUS_STARTING);

        new Thread(() -> {
            Process proc = null;
            try {
                File binary = binaryManager.getBinaryFile();
                if (!binary.exists()) {
                    synchronized (gatewayLock) {
                        isRunning.set(false); // 抢占回滚（进程未起，无条件清）
                    }
                    broadcastLog("ERROR: Binary not found at " + binary.getAbsolutePath());
                    broadcastStatus(STATUS_ERROR);
                    return;
                }

                File workDir = binaryManager.getWorkspaceDir().getParentFile();
                String workDirPath = workDir.getAbsolutePath();

                broadcastLog("Starting nemesisbot gateway...");
                broadcastLog("Binary: " + binary.getAbsolutePath());
                broadcastLog("WorkDir: " + workDirPath);

                ProcessBuilder pb = new ProcessBuilder(
                    binary.getAbsolutePath(),
                    "--local",
                    "gateway"
                );
                pb.directory(workDir);
                pb.redirectErrorStream(true);
                pb.environment().put("NEMESISBOT_HOME", workDirPath);

                synchronized (gatewayLock) {
                    if (!isRunning.get()) {
                        // 占位后被 stop 清旗标（起进程前的 onDestroy 窗口，
                        // stop 只能清旗标收不了尸）——此处放弃起进程，否则
                        // 服务销毁后进程无人收尸。
                        broadcastLog("Cancelled: service stopped before process start");
                        broadcastStatus(STATUS_STOPPED);
                        return;
                    }
                    proc = pb.start();
                    gatewayProcess = proc;
                }

                // Read stdout/stderr in a loop
                BufferedReader reader = new BufferedReader(
                    new InputStreamReader(proc.getInputStream())
                );

                String line;
                boolean foundListening = false;
                while ((line = reader.readLine()) != null) {
                    broadcastLog(line);

                    // Detect when server is ready
                    if (!foundListening && (line.contains("listening") || line.contains("Listening") || line.contains("started"))) {
                        foundListening = true;
                        broadcastStatus(STATUS_RUNNING);
                        updateNotification("NemesisBot 运行中");
                    }
                }

                int exitCode = proc.waitFor();
                synchronized (gatewayLock) {
                    // 只清自己的摊：自然退出与「新一轮 startGateway 抢占」
                    // 并发时，旧 worker 不得清掉新轮次的 running 占位。
                    if (gatewayProcess == proc) {
                        gatewayProcess = null;
                        isRunning.set(false);
                    }
                }
                broadcastLog("Process exited with code: " + exitCode);
                broadcastStatus(STATUS_STOPPED);

            } catch (Exception e) {
                synchronized (gatewayLock) {
                    if (gatewayProcess == proc) {
                        gatewayProcess = null;
                        isRunning.set(false);
                    }
                }
                broadcastLog("ERROR: " + e.getMessage());
                broadcastStatus(STATUS_ERROR);
                Log.e(TAG, "Gateway process error", e);
            }
        }, "gateway-thread").start();
    }

    /**
     * Stop the nemesisbot gateway process.
     */
    public void stopGateway() {
        synchronized (gatewayLock) {
            // S4（2026-10-14 审计）：与 startGateway 共用锁。旧实现双条件
            // `gatewayProcess != null && isRunning.get()`——onDestroy 落在
            // 「pb.start() 与 isRunning.set(true) 之间」时两条件不同真，
            // 跳过 destroyForcibly → 孤儿进程。现在只认 running 旗标；进程
            // 未起的窗口清旗标即可（worker 起进程前锁内复查会放弃启动）。
            if (!isRunning.get()) {
                return;
            }
            broadcastLog("Stopping nemesisbot...");
            if (gatewayProcess != null) {
                gatewayProcess.destroyForcibly();
                gatewayProcess = null;
            }
            isRunning.set(false);
            broadcastStatus(STATUS_STOPPED);
        }
        stopForeground(true);
        stopSelf();
    }

    private void broadcastLog(String line) {
        Log.d(TAG, line);
        Intent intent = new Intent(ACTION_LOG);
        intent.putExtra(EXTRA_LOG_LINE, line);
        intent.setPackage(getPackageName());
        sendBroadcast(intent);
    }

    private void broadcastStatus(String status) {
        Intent intent = new Intent(ACTION_STATUS);
        intent.putExtra(EXTRA_STATUS, status);
        intent.setPackage(getPackageName());
        sendBroadcast(intent);
    }

    /**
     * Acquire a non-refcounted MulticastLock so the WiFi driver stops
     * filtering broadcast/multicast packets while the gateway runs.
     * Without it, cluster UDP discovery announces from other nodes are
     * dropped by the driver and never reach the gateway's UDP socket.
     */
    private void acquireMulticastLock() {
        try {
            WifiManager wifi = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
            if (wifi == null) {
                broadcastLog("WARN: WifiManager unavailable, broadcast reception may be filtered");
                return;
            }
            multicastLock = wifi.createMulticastLock("nemesisbot-discovery");
            multicastLock.setReferenceCounted(false);
            multicastLock.acquire();
            broadcastLog("Multicast lock acquired (broadcast reception enabled)");
        } catch (Exception e) {
            multicastLock = null;
            broadcastLog("ERROR: Failed to acquire multicast lock: " + e.getMessage());
            Log.e(TAG, "acquireMulticastLock failed", e);
        }
    }

    private void releaseMulticastLock() {
        try {
            if (multicastLock != null && multicastLock.isHeld()) {
                multicastLock.release();
                broadcastLog("Multicast lock released");
            }
        } catch (Exception e) {
            Log.e(TAG, "releaseMulticastLock failed", e);
        } finally {
            multicastLock = null;
        }
    }

    private void createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel channel = new NotificationChannel(
                CHANNEL_ID,
                getString(R.string.notification_channel),
                NotificationManager.IMPORTANCE_LOW
            );
            channel.setDescription("NemesisBot gateway service notification");
            NotificationManager manager = getSystemService(NotificationManager.class);
            if (manager != null) {
                manager.createNotificationChannel(channel);
            }
        }
    }

    private Notification buildNotification(String text) {
        return new NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.notification_title))
            .setContentText(text)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setOngoing(true)
            .build();
    }

    private void updateNotification(String text) {
        NotificationManager nm = getSystemService(NotificationManager.class);
        if (nm != null) {
            nm.notify(NOTIFICATION_ID, buildNotification(text));
        }
    }
}
