<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'
import { useRouter } from 'vue-router'

const router = useRouter()
const transitionName = ref('slide-down')
const isTransitioning = ref(false)

// Track scroll direction based on route order
const routeOrder = ['chat', 'overview', 'persona', 'models', 'skills', 'cluster', 'settings', 'security', 'about']

let lastRouteIndex = 0

router.beforeEach((to, from) => {
  const fromIdx = routeOrder.indexOf(from.name as string)
  const toIdx = routeOrder.indexOf(to.name as string)
  // If navigating to a later route, slide up (new page comes from bottom)
  // If navigating to an earlier route, slide down (new page comes from top)
  transitionName.value = toIdx > fromIdx ? 'slide-up' : 'slide-down'
  lastRouteIndex = toIdx >= 0 ? toIdx : 0
})
</script>

<template>
  <router-view v-slot="{ Component }">
    <transition
      :name="transitionName"
      mode="out-in"
      @before-enter="isTransitioning = true"
      @after-leave="isTransitioning = false"
    >
      <div :key="$route.path" class="page-wrapper">
        <component :is="Component" />
      </div>
    </transition>
  </router-view>
</template>

<style>
/* ===== Page Wrapper ===== */
.page-wrapper {
  width: 100%;
  height: 100%;
  overflow-y: auto;
}

/* ===== Slide Up (new page from bottom) ===== */
.slide-up-enter-active,
.slide-up-leave-active {
  transition: all 200ms var(--ease-out);
}

.slide-up-enter-from {
  opacity: 0;
  transform: translateY(16px);
}

.slide-up-enter-to {
  opacity: 1;
  transform: translateY(0);
}

.slide-up-leave-from {
  opacity: 1;
  transform: translateY(0);
}

.slide-up-leave-to {
  opacity: 0;
  transform: translateY(-16px);
}

/* ===== Slide Down (new page from top) ===== */
.slide-down-enter-active,
.slide-down-leave-active {
  transition: all 200ms var(--ease-out);
}

.slide-down-enter-from {
  opacity: 0;
  transform: translateY(-16px);
}

.slide-down-enter-to {
  opacity: 1;
  transform: translateY(0);
}

.slide-down-leave-from {
  opacity: 1;
  transform: translateY(0);
}

.slide-down-leave-to {
  opacity: 0;
  transform: translateY(16px);
}

/* ===== Scale + Fade for extra smoothness ===== */
.slide-up-enter-active .page-content,
.slide-down-enter-active .page-content {
  animation: contentFadeIn 200ms var(--ease-out) forwards;
}

@keyframes contentFadeIn {
  from {
    opacity: 0;
    transform: scale(0.98);
  }
  to {
    opacity: 1;
    transform: scale(1);
  }
}
</style>
