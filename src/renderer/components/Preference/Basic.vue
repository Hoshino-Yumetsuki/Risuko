<template>
  <div class="content panel panel-layout panel-layout--v">
    <mo-enter tag="header" preset="fadeInDown" class="panel-header">
      <h4 class="hidden-xs-only">{{ title }}</h4>
      <div class="preference-mobile-subnav hidden-sm-and-up">
        <mo-subnav-switcher :title="title" :subnavs="subnavs" />
      </div>
    </mo-enter>
    <main class="panel-content">
      <form class="form-preference" ref="basicForm" @submit.prevent>
        <!-- Appearance Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Palette :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.appearance') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div style="margin-bottom: 16px">
              <mo-theme-switcher
                v-model="form.theme"
                @change="handleThemeChange"
                ref="themeSwitcher"
              />
            </div>
            <div class="typography-controls">
              <div v-if="!isAndroid" class="typography-row typography-row--font">
                <div class="typography-row-main">
                  <label class="settings-select-item-label">{{ $t('preferences.font-family') }}</label>
                  <Select v-model="form.fontFamily" class="typography-font-select">
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="item in fontFamilyOptions" :key="item.value" :value="item.value">
                        {{ item.label }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div
                  class="typography-sample"
                  :class="`typography-sample--${form.fontFamily}`"
                >
                  {{ $t('preferences.font-family-sample') }}
                </div>
              </div>
              <div class="typography-row typography-row--size">
                <div class="typography-row-main">
                  <span class="settings-select-item-label">{{ $t('preferences.font-size') }}</span>
                </div>
                <div class="font-size-segmented" role="radiogroup" :aria-label="$t('preferences.font-size')">
                  <button
                    v-for="item in fontSizeOptions"
                    :key="item.value"
                    type="button"
                    role="radio"
                    class="font-size-segment"
                    :class="{ 'font-size-segment--active': form.fontSize === item.value }"
                    :aria-label="item.label"
                    :aria-checked="form.fontSize === item.value"
                    @click="form.fontSize = item.value"
                  >
                    {{ item.shortLabel }}
                  </button>
                </div>
              </div>
            </div>
            <div v-if="showHideAppMenuOption" class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.hide-app-menu') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.hideAppMenu"
                  @change="(val) => setBasicBoolean('hideAppMenu', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.auto-hide-window') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.autoHideWindow"
                  @change="(val) => setBasicBoolean('autoHideWindow', val)"
                />
              </div>
            </div>
            <div v-if="isMac" class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.tray-speedometer') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.traySpeedometer"
                  @change="(val) => setBasicBoolean('traySpeedometer', val)"
                />
              </div>
            </div>
          </div>
        </div>

        <!-- Language & Startup Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Globe :size="16" /></div>
            <div class="section-title">
              <h3>
                {{ $t('preferences.language') }} &
                {{ $t('preferences.startup') }}
              </h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.language') }}</label>
                <Select v-model="form.locale" class="settings-select-control">
                  <SelectTrigger>
                    <SelectValue :placeholder="$t('preferences.change-language')" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem v-for="item in locales" :key="item.value" :value="item.value">
                      {{ item.label }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div v-if="isMac" class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.run-mode') }}</label>
                <Select v-model="form.runMode" class="settings-select-control">
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem v-for="item in runModes" :key="item.value" :value="item.value">
                      {{ item.label }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.open-at-login') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.openAtLogin"
                  @change="(val) => setBasicBoolean('openAtLogin', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.keep-window-state') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.keepWindowState"
                  @change="(val) => setBasicBoolean('keepWindowState', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.auto-resume-all') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.resumeAllWhenAppLaunched"
                  @change="(val) => setBasicBoolean('resumeAllWhenAppLaunched', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.purge-record-on-start') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.purgeRecordOnStart"
                  @change="(val) => setBasicBoolean('purgeRecordOnStart', val)"
                />
              </div>
            </div>
          </div>
        </div>

        <!-- Download Location Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><FolderDown :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.default-dir') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="mo-input-group mo-input-group--bordered">
              <span class="mo-input-prepend">
                <mo-history-directory @selected="handleHistoryDirectorySelected" />
              </span>
              <Input
                placeholder=""
                v-model="form.dir"
                readonly
                class="path-indicator-field flex-1 shadow-none rounded-none border-none noinput"
              />
              <span class="mo-input-append" v-if="isRenderer">
                <mo-select-directory @selected="handleNativeDirectorySelected" />
              </span>
            </div>
            <div class="form-info" v-if="isMas">
              {{ $t('preferences.mas-default-dir-tips') }}
            </div>
          </div>
        </div>

        <!-- File Category Paths Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><FolderDown :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.file-category-dirs') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="form-info" style="margin-bottom: 8px">
              {{ $t('preferences.file-category-dirs-tips') }}
            </div>
            <div
              v-for="cat in fileCategories"
              :key="cat.key"
              class="settings-row category-path-row"
              style="margin-bottom: 6px"
            >
              <span class="settings-row-title category-path-label" style="flex: 0 0 80px; min-width: 80px">{{
                cat.label
              }}</span>
              <div class="mo-input-group mo-input-group--bordered category-path-group" style="flex: 1; min-width: 0">
                <Input
                  :model-value="categoryDirectoryValue(cat.key)"
                  readonly
                  class="path-indicator-field flex-1 shadow-none rounded-none border-none noinput"
                />
                <span class="mo-input-append" v-if="isRenderer">
                  <mo-select-directory
                    @selected="(dir) => handleCategoryDirectorySelected(cat.key, dir)"
                  />
                </span>
              </div>
            </div>
          </div>
        </div>

        <!-- Task Routing Rules Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><FolderDown :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.task-routing-rules') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="form-info" style="margin-bottom: 8px">
              {{ $t('preferences.task-routing-rules-tips') }}
            </div>
            <div
              v-for="(rule, index) in form.taskRoutingRules"
              :key="rule.id"
              class="settings-row"
              style="margin-bottom: 6px; align-items: flex-start"
            >
              <div style="flex: 1; display: flex; gap: 6px; min-width: 0; flex-wrap: wrap">
                <Input
                  :placeholder="$t('preferences.task-routing-rule-pattern-placeholder')"
                  :model-value="form.taskRoutingRules[index].pattern"
                  @update:model-value="(val) => updateRuleField(index, 'pattern', val)"
                  class="flex-1 shadow-none border-none"
                  style="min-width: 100px"
                />
                <Input
                  :placeholder="$t('preferences.task-routing-rule-label-placeholder')"
                  :model-value="form.taskRoutingRules[index].label"
                  @update:model-value="(val) => updateRuleField(index, 'label', val)"
                  class="flex-1 shadow-none border-none"
                  style="min-width: 80px"
                />
                <div class="mo-input-group mo-input-group--bordered" style="flex: 1; min-width: 160px">
                  <Input
                    :placeholder="$t('preferences.task-routing-rule-dir-placeholder')"
                    :model-value="form.taskRoutingRules[index].dir"
                    @update:model-value="(val) => updateRuleField(index, 'dir', val)"
                    class="path-indicator-field flex-1 shadow-none rounded-none border-none"
                  />
                  <span class="mo-input-append" v-if="isRenderer">
                    <mo-select-directory
                      class="routing-rule-dir-picker"
                      @selected="(dir) => handleRoutingRuleDirectorySelected(index, dir)"
                    />
                  </span>
                </div>
                <div class="settings-row-action" style="flex: 0 0 auto">
                  <ui-checkbox
                    :model-value="!!form.taskRoutingRules[index].enabled"
                    @change="(val) => updateRuleField(index, 'enabled', !!val)"
                  />
                </div>
                <ui-button
                  size="mini"
                  variant="text"
                  @click="removeRoutingRule(index)"
                  style="padding: 4px 8px"
                >
                  ×
                </ui-button>
              </div>
            </div>
            <div class="settings-row">
              <ui-button size="mini" variant="primary" @click="addRoutingRule">
                + {{ $t('preferences.task-routing-rule-add') }}
              </ui-button>
            </div>
          </div>
        </div>

        <!-- Transfer Speed Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Gauge :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.transfer-settings') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label"
                  ><ArrowUp :size="12" style="vertical-align: middle; margin-right: 4px" />{{
                    $t('preferences.transfer-speed-upload')
                  }}</label
                >
                <div class="settings-inline-input">
                  <NumberInput
                    v-model="maxOverallUploadLimitParsed"
                    :min="0"
                    :max="65535"
                    :step="1"
                  />
                  <Select v-model="uploadUnit" @update:model-value="handleUploadChange">
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="item in speedUnits" :key="item.value" :value="item.value">
                        {{ item.label }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label"
                  ><ArrowDown :size="12" style="vertical-align: middle; margin-right: 4px" />{{
                    $t('preferences.transfer-speed-download')
                  }}</label
                >
                <div class="settings-inline-input">
                  <NumberInput
                    v-model="maxOverallDownloadLimitParsed"
                    :min="0"
                    :max="65535"
                    :step="1"
                  />
                  <Select v-model="downloadUnit" @update:model-value="handleDownloadChange">
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="item in speedUnits" :key="item.value" :value="item.value">
                        {{ item.label }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- BitTorrent Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Share2 :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.bt-settings') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.bt-save-metadata') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.btSaveMetadata"
                  @change="(val) => setBasicBoolean('btSaveMetadata', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.bt-force-encryption') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.btForceEncryption"
                  @change="(val) => setBasicBoolean('btForceEncryption', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.keep-seeding') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox :model-value="!!form.keepSeeding" @change="onKeepSeedingToggle" />
              </div>
            </div>
            <div v-if="form.keepSeeding" class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.seed-ratio') }}</label>
                <NumberInput v-model="form.seedRatio" :min="0" :max="100" :step="0.1" />
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label"
                  >{{ $t('preferences.seed-time') }} ({{ $t('preferences.seed-time-unit') }})</label
                >
                <NumberInput v-model="form.seedTime" :min="0" :max="525600" :step="1" />
              </div>
            </div>
          </div>
        </div>

        <!-- Task Management Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><ListTodo :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.task-manage') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{
                  $t('preferences.max-concurrent-downloads')
                }}</label>
                <NumberInput
                  v-model="form.maxConcurrentDownloads"
                  :min="1"
                  :max="maxConcurrentDownloads"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.auto-retry') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.autoRetry"
                  @change="(val) => setBasicBoolean('autoRetry', val)"
                />
              </div>
            </div>
            <div v-if="form.autoRetry" class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{
                  $t('preferences.auto-retry-strategy')
                }}</label>
                <Select v-model="form.autoRetryStrategy" class="settings-select-control">
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem
                      v-for="item in retryStrategies"
                      :key="item.value"
                      :value="item.value"
                    >
                      {{ item.label }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">
                  {{ $t('preferences.auto-retry-interval') }} ({{
                    $t('preferences.auto-retry-interval-unit')
                  }})
                </label>
                <NumberInput v-model="form.autoRetryInterval" :min="1" :max="300" :step="1" />
              </div>
            </div>
            <div v-if="form.autoRetry" class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">
                  {{ $t('preferences.worker-max-retries') }}
                </label>
                <NumberInput v-model="form.workerMaxRetries" :min="1" :max="20" :step="1" />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.auto-detect-low-speed-tasks') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.auto-detect-low-speed-tasks-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.autoDetectLowSpeedTasks"
                  @change="(val) => setBasicBoolean('autoDetectLowSpeedTasks', val)"
                />
              </div>
            </div>
            <div v-if="form.autoDetectLowSpeedTasks" class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">
                  {{ $t('preferences.low-speed-threshold') }} ({{
                    $t('preferences.low-speed-threshold-unit')
                  }})
                </label>
                <NumberInput v-model="form.lowSpeedThreshold" :min="1" :max="10240" :step="1" />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{
                  $t('preferences.new-task-show-downloading')
                }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.newTaskShowDownloading"
                  @change="(val) => setBasicBoolean('newTaskShowDownloading', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{
                  $t('preferences.task-completed-notify')
                }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.taskNotification"
                  @change="(val) => setBasicBoolean('taskNotification', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.prevent-sleep-while-downloading') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.prevent-sleep-while-downloading-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.preventSleepWhileDownloading"
                  @change="(val) => setBasicBoolean('preventSleepWhileDownloading', val)"
                />
              </div>
            </div>
            <div v-if="!isAndroid" class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.shutdown-when-complete') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.shutdown-when-complete-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.shutdownWhenComplete"
                  @change="(val) => setBasicBoolean('shutdownWhenComplete', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{
                  $t('preferences.no-confirm-before-delete-task')
                }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.noConfirmBeforeDeleteTask"
                  @change="(val) => setBasicBoolean('noConfirmBeforeDeleteTask', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.use-remote-file-time') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.use-remote-file-time-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.useRemoteFileTime"
                  @change="(val) => setBasicBoolean('useRemoteFileTime', val)"
                />
              </div>
            </div>
          </div>
        </div>

        <!-- Version Info Section -->
        <div class="settings-section">
          <div class="settings-section-header"></div>
          <div class="settings-section-content version-section">
            <div class="version-indicator">
              <div class="version-item">
                <span class="version-name">Risuko</span>
                <span class="version-value">{{ appVersion || '--' }}</span>
              </div>
              <div class="version-item">
                <span class="version-name">Engine</span>
                <span class="version-value">{{ engineVersion }}</span>
              </div>
            </div>
          </div>
        </div>
      </form>
      <div class="form-actions">
        <ui-button @click="resetForm('basicForm')">{{ $t('preferences.discard') }}</ui-button>
        <ui-button variant="primary" @click="submitForm('basicForm')">{{
          $t('preferences.save')
        }}</ui-button>
      </div>
    </main>
  </div>
</template>

<script lang="ts">
import {
	ArrowDown,
	ArrowUp,
	FolderDown,
	Gauge,
	Globe,
	ListTodo,
	Palette,
	Share2,
} from "@lucide/vue";
import {
	APP_RUN_MODE,
	EMPTY_STRING,
	ENGINE_MAX_CONCURRENT_DOWNLOADS,
	ENGINE_RPC_PORT,
	FILE_CATEGORIES,
} from "@shared/constants";
import { availableLanguages } from "@shared/locales";
import {
	DEFAULT_FONT_FAMILY,
	DEFAULT_FONT_SIZE,
	FONT_FAMILY_OPTIONS,
	FONT_SIZE_OPTIONS,
	normalizeConfigOption,
} from "@shared/types/config";
import {
	changedConfig,
	convertLineToComma,
	diffConfig,
	extractSpeedUnit,
	parseBooleanConfig,
} from "@shared/utils";
import logger from "@shared/utils/logger";
import { reduceTrackerString } from "@shared/utils/tracker";
import { invoke } from "@tauri-apps/api/core";
import { cloneDeep, extend, isEmpty } from "lodash";
import SelectDirectory from "@/components/Native/SelectDirectory.vue";
import HistoryDirectory from "@/components/Preference/HistoryDirectory.vue";
import ThemeSwitcher from "@/components/Preference/ThemeSwitcher.vue";
import SubnavSwitcher from "@/components/Subnav/SubnavSwitcher.vue";
import UiButton from "@/components/ui/compat/UiButton.vue";
import { confirm } from "@/components/ui/confirm-dialog";
import { Input } from "@/components/ui/input";
import NumberInput from "@/components/ui/NumberInput.vue";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import is from "@/shims/platform";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";
import { getRisukoVersion } from "@/utils/version";

const RETRY_STRATEGY_STATIC = "static";
const RETRY_STRATEGY_EXPONENTIAL = "exponential";

const normalizePositiveInt = (
	value,
	fallback,
	min = 1,
	max = Number.MAX_SAFE_INTEGER,
) => {
	const parsed = Number(value);
	if (!Number.isFinite(parsed)) {
		return fallback;
	}
	return Math.min(Math.max(Math.floor(parsed), min), max);
};

const initForm = (config) => {
	const {
		autoDetectLowSpeedTasks,
		autoRetry,
		autoRetryInterval,
		autoRetryStrategy,
		workerMaxRetries,
		autoHideWindow,
		btForceEncryption,
		btSaveMetadata,
		dir,
		fileCategoryDirs,
		fontFamily,
		fontSize,
		followTorrent,
		hideAppMenu,
		keepSeeding,
		keepWindowState,
		locale,
		maxConcurrentDownloads,
		maxOverallDownloadLimit,
		maxOverallUploadLimit,
		newTaskShowDownloading,
		noConfirmBeforeDeleteTask,
		openAtLogin,
		preventSleepWhileDownloading,
		shutdownWhenComplete,
		purgeRecordOnStart,
		resumeAllWhenAppLaunched,
		runMode,
		seedRatio,
		seedTime,
		showProgressBar,
		taskNotification,
		theme,
		traySpeedometer,
		useRemoteFileTime,
		lowSpeedThreshold,
	} = config;

	const result = {
		autoDetectLowSpeedTasks: parseBooleanConfig(autoDetectLowSpeedTasks),
		autoRetry: parseBooleanConfig(autoRetry),
		autoRetryInterval: normalizePositiveInt(autoRetryInterval, 5, 1, 300),
		autoRetryStrategy:
			autoRetryStrategy === RETRY_STRATEGY_EXPONENTIAL
				? RETRY_STRATEGY_EXPONENTIAL
				: RETRY_STRATEGY_STATIC,
		workerMaxRetries: normalizePositiveInt(workerMaxRetries, 5, 1, 20),
		autoHideWindow: parseBooleanConfig(autoHideWindow),
		btForceEncryption: parseBooleanConfig(btForceEncryption),
		btSaveMetadata: parseBooleanConfig(btSaveMetadata),
		continue: parseBooleanConfig(config.continue),
		dir,
		fileCategoryDirs: {
			music: "",
			video: "",
			image: "",
			document: "",
			compressed: "",
			program: "",
			rss: "",
			...(fileCategoryDirs || {}),
		},
		fontFamily: normalizeConfigOption(
			fontFamily,
			FONT_FAMILY_OPTIONS,
			DEFAULT_FONT_FAMILY,
		),
		fontSize: normalizeConfigOption(
			fontSize,
			FONT_SIZE_OPTIONS,
			DEFAULT_FONT_SIZE,
		),
		taskRoutingRules: (config.taskRoutingRules || []).map((rule) => ({
			...rule,
			id: rule.id || crypto.randomUUID(),
		})),
		followTorrent,
		hideAppMenu: parseBooleanConfig(hideAppMenu),
		keepSeeding: parseBooleanConfig(keepSeeding),
		keepWindowState: parseBooleanConfig(keepWindowState),
		locale,
		lowSpeedThreshold: normalizePositiveInt(lowSpeedThreshold, 20, 1, 10240),
		maxConcurrentDownloads,
		maxOverallDownloadLimit,
		maxOverallUploadLimit,
		newTaskShowDownloading: parseBooleanConfig(newTaskShowDownloading),
		noConfirmBeforeDeleteTask: parseBooleanConfig(noConfirmBeforeDeleteTask),
		openAtLogin: parseBooleanConfig(openAtLogin),
		preventSleepWhileDownloading:
			preventSleepWhileDownloading === undefined
				? false
				: parseBooleanConfig(preventSleepWhileDownloading),
		shutdownWhenComplete: parseBooleanConfig(shutdownWhenComplete),
		purgeRecordOnStart: parseBooleanConfig(purgeRecordOnStart),
		resumeAllWhenAppLaunched: parseBooleanConfig(resumeAllWhenAppLaunched),
		runMode,
		seedRatio,
		seedTime,
		showProgressBar: parseBooleanConfig(showProgressBar),
		taskNotification: parseBooleanConfig(taskNotification),
		theme,
		traySpeedometer: parseBooleanConfig(traySpeedometer),
		useRemoteFileTime: parseBooleanConfig(useRemoteFileTime),
	};
	return result;
};

export default {
	name: "mo-preference-basic",
	components: {
		[SubnavSwitcher.name]: SubnavSwitcher,
		[HistoryDirectory.name]: HistoryDirectory,
		[SelectDirectory.name]: SelectDirectory,
		[ThemeSwitcher.name]: ThemeSwitcher,
		[UiButton.name]: UiButton,
		NumberInput,
		Input,
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
		Palette,
		Globe,
		FolderDown,
		Gauge,
		Share2,
		ListTodo,
		ArrowUp,
		ArrowDown,
	},
	data() {
		const preferenceStore = usePreferenceStore();
		const formOriginal = initForm(preferenceStore.config);
		let form = {};
		form = initForm(extend(form, formOriginal, changedConfig.basic));

		return {
			appVersion: "",
			form,
			formOriginal,
			locales: availableLanguages,
		};
	},
	created() {
		getRisukoVersion().then((v) => {
			this.appVersion = v;
		});

		const currentEngineVersion = this.engineInfo?.version;
		if (!currentEngineVersion) {
			useAppStore().fetchEngineInfo();
		}
	},
	computed: {
		isRenderer: () => is.renderer(),
		isMac: () => is.macOS(),
		isMas: () => is.mas(),
		isAndroid: () => is.android(),
		title() {
			return this.$t("preferences.basic");
		},
		maxConcurrentDownloads() {
			return ENGINE_MAX_CONCURRENT_DOWNLOADS;
		},
		fileCategories() {
			return Object.values(FILE_CATEGORIES).map((key) => ({
				key,
				label: this.$t(`preferences.file-category-${key}`),
			}));
		},
		maxOverallDownloadLimitParsed: {
			get() {
				return parseInt(this.form.maxOverallDownloadLimit, 10);
			},
			set(value) {
				const limit = value > 0 ? `${value}${this.downloadUnit}` : 0;
				this.form.maxOverallDownloadLimit = limit;
			},
		},
		maxOverallUploadLimitParsed: {
			get() {
				return parseInt(this.form.maxOverallUploadLimit, 10);
			},
			set(value) {
				const limit = value > 0 ? `${value}${this.uploadUnit}` : 0;
				this.form.maxOverallUploadLimit = limit;
			},
		},
		downloadUnit: {
			get() {
				const { maxOverallDownloadLimit } = this.form;
				return extractSpeedUnit(maxOverallDownloadLimit);
			},
			set(value) {
				return value;
			},
		},
		uploadUnit: {
			get() {
				const { maxOverallUploadLimit } = this.form;
				return extractSpeedUnit(maxOverallUploadLimit);
			},
			set(value) {
				return value;
			},
		},
		runModes() {
			const result = [
				{
					label: this.$t("preferences.run-mode-standard"),
					value: APP_RUN_MODE.STANDARD,
				},
				{
					label: this.$t("preferences.run-mode-tray"),
					value: APP_RUN_MODE.TRAY,
				},
			];
			return result;
		},
		fontFamilyOptions() {
			return FONT_FAMILY_OPTIONS.map((value) => ({
				label: this.$t(`preferences.font-family-${value}`),
				value,
			}));
		},
		fontSizeOptions() {
			return FONT_SIZE_OPTIONS.map((value) => ({
				label: this.$t(`preferences.font-size-${value}`),
				shortLabel: this.$t(`preferences.font-size-${value}-short`),
				value,
			}));
		},
		speedUnits() {
			return [
				{
					label: "KB/s",
					value: "K",
				},
				{
					label: "MB/s",
					value: "M",
				},
			];
		},
		retryStrategies() {
			return [
				{
					label: this.$t("preferences.auto-retry-strategy-static"),
					value: RETRY_STRATEGY_STATIC,
				},
				{
					label: this.$t("preferences.auto-retry-strategy-exponential"),
					value: RETRY_STRATEGY_EXPONENTIAL,
				},
			];
		},
		subnavs() {
			return [
				{
					key: "basic",
					title: this.$t("preferences.basic"),
					route: "/preference/basic",
				},
				{
					key: "advanced",
					title: this.$t("preferences.advanced"),
					route: "/preference/advanced",
				},
			];
		},
		showHideAppMenuOption() {
			return is.windows() || is.linux();
		},
		rpcDefaultPort() {
			return ENGINE_RPC_PORT;
		},
		engineVersion() {
			const engineVersion = this.engineInfo?.version;
			return engineVersion ? `${engineVersion}` : "--";
		},
		engineInfo() {
			return useAppStore().engineInfo;
		},
	},
	methods: {
		setBasicBoolean(key, enable) {
			this.form[key] = !!enable;
		},
		categoryDirectoryValue(category) {
			return this.form.fileCategoryDirs?.[category] || this.form.dir || "";
		},
		handleCategoryDirectorySelected(category, dir) {
			this.form.fileCategoryDirs = {
				...this.form.fileCategoryDirs,
				[category]: dir,
			};
		},
		handleRoutingRuleDirectorySelected(index, dir) {
			this.updateRuleField(index, "dir", dir);
		},
		addRoutingRule() {
			const rules = [...(this.form.taskRoutingRules || [])];
			rules.push({
				id: crypto.randomUUID(),
				label: "",
				pattern: "",
				dir: "",
				enabled: true,
			});
			this.form.taskRoutingRules = rules;
		},
		removeRoutingRule(index) {
			const rules = [...(this.form.taskRoutingRules || [])];
			rules.splice(index, 1);
			this.form.taskRoutingRules = rules;
		},
		updateRuleField(index, field, value) {
			const rules = [...this.form.taskRoutingRules];
			rules[index] = { ...rules[index], [field]: value };
			this.form.taskRoutingRules = rules;
		},
		handleThemeChange(theme) {
			this.form.theme = theme;
		},
		handleDownloadChange(value) {
			const speedLimit = parseInt(this.form.maxOverallDownloadLimit, 10);
			this.downloadUnit = value;
			const limit = speedLimit > 0 ? `${speedLimit}${value}` : 0;
			this.form.maxOverallDownloadLimit = limit;
		},
		handleUploadChange(value) {
			const speedLimit = parseInt(this.form.maxOverallUploadLimit, 10);
			this.uploadUnit = value;
			const limit = speedLimit > 0 ? `${speedLimit}${value}` : 0;
			this.form.maxOverallUploadLimit = limit;
		},
		onKeepSeedingChange(enable) {
			if (!enable) {
				this.form.seedRatio = 0;
			}
			this.form.seedTime = enable ? 525600 : 0;
		},
		onKeepSeedingToggle(enable) {
			this.form.keepSeeding = !!enable;
			this.onKeepSeedingChange(this.form.keepSeeding);
		},
		handleHistoryDirectorySelected(dir) {
			this.form.dir = dir;
		},
		handleNativeDirectorySelected(dir) {
			this.form.dir = dir;
			usePreferenceStore().recordHistoryDirectory(dir);
		},
		syncFormConfig() {
			usePreferenceStore()
				.fetchPreference()
				.then((config) => {
					this.form = initForm(config);
					this.formOriginal = cloneDeep(this.form);
				});
		},
		submitForm(_formName) {
			const data = {
				...diffConfig(this.formOriginal, this.form),
				...changedConfig.advanced,
			};
			const booleanKeys = [
				"hideAppMenu",
				"autoHideWindow",
				"traySpeedometer",
				"showProgressBar",
				"openAtLogin",
				"keepWindowState",
				"resumeAllWhenAppLaunched",
				"purgeRecordOnStart",
				"btSaveMetadata",
				"btForceEncryption",
				"keepSeeding",
				"continue",
				"autoRetry",
				"autoDetectLowSpeedTasks",
				"newTaskShowDownloading",
				"preventSleepWhileDownloading",
				"shutdownWhenComplete",
				"taskNotification",
				"noConfirmBeforeDeleteTask",
				"useRemoteFileTime",
			];
			for (const key of booleanKeys) {
				if (key in data) {
					data[key] = !!this.form[key];
				}
			}

			const { autoHideWindow, btTracker, rpcListenPort } = data;

			if (btTracker) {
				data.btTracker = reduceTrackerString(convertLineToComma(btTracker));
			}

			if (rpcListenPort === EMPTY_STRING) {
				data.rpcListenPort = this.rpcDefaultPort;
			}

			if ("autoRetryInterval" in data) {
				data.autoRetryInterval = normalizePositiveInt(
					this.form.autoRetryInterval,
					5,
					1,
					300,
				);
			}

			if ("lowSpeedThreshold" in data) {
				data.lowSpeedThreshold = normalizePositiveInt(
					this.form.lowSpeedThreshold,
					20,
					1,
					10240,
				);
			}

			if ("autoRetryStrategy" in data) {
				data.autoRetryStrategy =
					this.form.autoRetryStrategy === RETRY_STRATEGY_EXPONENTIAL
						? RETRY_STRATEGY_EXPONENTIAL
						: RETRY_STRATEGY_STATIC;
			}

			if ("workerMaxRetries" in data) {
				data.workerMaxRetries = normalizePositiveInt(
					this.form.workerMaxRetries,
					5,
					1,
					20,
				);
			}

			if ("fontFamily" in data) {
				data.fontFamily = normalizeConfigOption(
					this.form.fontFamily,
					FONT_FAMILY_OPTIONS,
					DEFAULT_FONT_FAMILY,
				);
			}

			if ("fontSize" in data) {
				data.fontSize = normalizeConfigOption(
					this.form.fontSize,
					FONT_SIZE_OPTIONS,
					DEFAULT_FONT_SIZE,
				);
			}

			logger.log("[Risuko] preference changed data:", data);

			usePreferenceStore()
				.save(data)
				.then(() => {
					this.syncFormConfig();
					this.$msg.success(this.$t("preferences.save-success-message"));
					if (this.isRenderer) {
						if ("autoHideWindow" in data) {
							invoke("auto_hide_window", { enabled: autoHideWindow }).catch(
								() => {
									/* noop */
								},
							);
						}
						if ("hideAppMenu" in data) {
							invoke("toggle_app_menu", {
								hidden: !!data.hideAppMenu,
							}).catch(() => {
								/* noop */
							});
						}
					}
					changedConfig.basic = {};
					changedConfig.advanced = {};
				})
				.catch(() => {
					this.$msg.error(this.$t("preferences.save-fail-message"));
				});
		},
		resetForm(_formName) {
			this.syncFormConfig();
		},
	},
	async beforeRouteLeave(to, _from) {
		changedConfig.basic = diffConfig(this.formOriginal, this.form);
		if (to.path === "/preference/advanced") {
			return true;
		}
		if (isEmpty(changedConfig.basic) && isEmpty(changedConfig.advanced)) {
			return true;
		}
		const { confirmed } = await confirm({
			message: this.$t("preferences.not-saved-confirm"),
			title: this.$t("preferences.not-saved"),
			kind: "warning",
			confirmText: this.$t("app.yes"),
			cancelText: this.$t("app.no"),
		});
		if (confirmed) {
			changedConfig.basic = {};
			changedConfig.advanced = {};
			return true;
		}
		return false;
	},
};
</script>
