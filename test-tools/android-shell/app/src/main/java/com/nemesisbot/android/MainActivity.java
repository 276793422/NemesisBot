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

    private ConfigFragment configFragment;
    private DashboardFragment dashboardFragment;
    private Fragment activeFragment;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_main);

        configFragment = new ConfigFragment();
        dashboardFragment = new DashboardFragment();

        // Show config fragment by default
        getSupportFragmentManager().beginTransaction()
            .add(R.id.fragment_container, dashboardFragment, "dashboard")
            .hide(dashboardFragment)
            .add(R.id.fragment_container, configFragment, "config")
            .commit();
        activeFragment = configFragment;

        BottomNavigationView bottomNav = findViewById(R.id.bottom_navigation);
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

        Log.i(TAG, "MainActivity created");
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
