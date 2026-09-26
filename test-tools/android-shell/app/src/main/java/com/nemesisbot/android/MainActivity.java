package com.nemesisbot.android;

import androidx.annotation.NonNull;
import androidx.appcompat.app.AppCompatActivity;
import androidx.fragment.app.Fragment;

import android.os.Bundle;
import android.util.Log;

import com.google.android.material.bottomnavigation.BottomNavigationView;

/**
 * Main Activity with bottom navigation between Config and Dashboard pages.
 */
public class MainActivity extends AppCompatActivity {
    private static final String TAG = "MainActivity";
    private static final String KEY_ACTIVE_FRAGMENT = "active_fragment";

    private ConfigFragment configFragment;
    private DashboardFragment dashboardFragment;
    private Fragment activeFragment;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_main);

        if (savedInstanceState != null) {
            // Activity recreation (process death restore etc.): FragmentManager
            // already restored the fragment instances — re-find them by tag
            // instead of adding a second copy on top (duplicate stack caused
            // rotation landing back on config page + black/broken dashboard).
            configFragment = (ConfigFragment) getSupportFragmentManager().findFragmentByTag("config");
            dashboardFragment = (DashboardFragment) getSupportFragmentManager().findFragmentByTag("dashboard");
        }
        if (configFragment != null && dashboardFragment != null) {
            String activeTag = savedInstanceState != null
                ? savedInstanceState.getString(KEY_ACTIVE_FRAGMENT, "config") : "config";
            activeFragment = "dashboard".equals(activeTag) ? dashboardFragment : configFragment;
        } else {
            // Fresh create: first launch, or a saved state that predates the
            // initial fragment transaction actually executing (commit() is
            // async, so a kill within that window restores nothing).
            configFragment = new ConfigFragment();
            dashboardFragment = new DashboardFragment();

            // Show config fragment by default
            getSupportFragmentManager().beginTransaction()
                .add(R.id.fragment_container, dashboardFragment, "dashboard")
                .hide(dashboardFragment)
                .add(R.id.fragment_container, configFragment, "config")
                .commit();
            activeFragment = configFragment;
        }

        BottomNavigationView bottomNav = findViewById(R.id.bottom_navigation);

        // Keep bottom nav selection in sync with the restored active fragment
        // (before attaching the listener so it doesn't re-trigger a switch).
        if (activeFragment == dashboardFragment) {
            bottomNav.setSelectedItemId(R.id.nav_dashboard);
        }

        bottomNav.setOnItemSelectedListener(item -> {
            int id = item.getItemId();
            if (id == R.id.nav_config) {
                switchFragment(configFragment);
                return true;
            } else if (id == R.id.nav_dashboard) {
                switchFragment(dashboardFragment);
                return true;
            }
            return false;
        });

        Log.i(TAG, "MainActivity created (restored=" + (savedInstanceState != null) + ")");
    }

    @Override
    protected void onSaveInstanceState(@NonNull Bundle outState) {
        super.onSaveInstanceState(outState);
        outState.putString(KEY_ACTIVE_FRAGMENT,
            activeFragment == dashboardFragment ? "dashboard" : "config");
    }

    private void switchFragment(@NonNull Fragment target) {
        if (target == activeFragment) return;
        getSupportFragmentManager().beginTransaction()
            .hide(activeFragment)
            .show(target)
            .commit();
        activeFragment = target;
    }
}
