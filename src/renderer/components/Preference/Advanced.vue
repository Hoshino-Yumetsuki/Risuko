<template>
  <div class="content panel panel-layout panel-layout--v">
    <header class="panel-header">
      <h4 class="hidden-xs-only">{{ title }}</h4>
      <mo-subnav-switcher :title="title" :subnavs="subnavs" class="hidden-sm-and-up" />
    </header>
    <main class="panel-content">
      <form class="form-preference" ref="advancedForm" @submit.prevent>
        <!-- Auto Update Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><RefreshCw :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.auto-update') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.auto-check-update') }}
                </div>
                <div class="settings-row-description" v-if="form.lastCheckUpdateTime !== 0">
                  {{ $t('preferences.last-check-update-time') }}:
                  {{ new Date(form.lastCheckUpdateTime).toLocaleString() }}
                  <span
                    class="action-link"
                    @click.prevent="onCheckUpdateClick"
                    style="margin-left: 8px"
                  >
                    {{ $t('app.check-updates-now') }}
                  </span>
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.autoCheckUpdate"
                  @change="(val) => setAdvancedBoolean('autoCheckUpdate', val)"
                />
              </div>
            </div>
          </div>
        </div>

        <!-- Proxy Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Globe :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.proxy') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.enable-proxy') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox :model-value="form.proxy.enable" @change="onProxyEnableChange" />
              </div>
            </div>
            <div v-if="form.proxy.enable" style="margin-top: 14px">
              <div class="form-item-sub" style="margin-bottom: 10px">
                <Input
                  placeholder="[http://][USER:PASSWORD@]HOST[:PORT]"
                  v-model="form.proxy.server"
                />
              </div>
              <div class="form-item-sub" style="margin-bottom: 10px">
                <Textarea
                  :rows="2"
                  autocomplete="off"
                  :placeholder="`${$t('preferences.proxy-bypass-input-tips')}`"
                  v-model="form.proxy.bypass"
                />
              </div>
              <div class="form-item-sub proxy-scope-group">
                <label class="settings-select-item-label">{{
                  $t('preferences.proxy-scope-label')
                }}</label>
                <div v-for="item in proxyScopeOptions" :key="item" class="proxy-scope-item">
                  <ui-checkbox
                    :model-value="form.proxy.scope.includes(item)"
                    @change="(val) => onProxyScopeToggle(item, val)"
                  >
                    {{ $t(`preferences.proxy-scope-${item}`) }}
                  </ui-checkbox>
                  <div class="proxy-scope-desc">
                    {{ $t(`preferences.proxy-scope-${item}-desc`) }}
                  </div>
                </div>
                <div class="form-info" style="margin-top: 8px">
                  <a
                    target="_blank"
                    href="https://github.com/agalwood/Motrix/wiki/Proxy"
                    rel="noopener noreferrer"
                  >
                    {{ $t('preferences.proxy-tips') }}
                    <ExternalLink :size="12" />
                  </a>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- BT Tracker Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Radio :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.bt-tracker') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="bt-tracker">
              <label class="settings-select-item-label" style="margin-bottom: 6px"
                >{{ $t('preferences.bt-tracker') }} Source</label
              >
              <div class="bt-tracker-source-row">
                <Popover v-model:open="trackerSourceOpen">
                  <PopoverTrigger as-child>
                    <button
                      type="button"
                      class="tracker-multi-select-trigger"
                      role="combobox"
                      :aria-expanded="trackerSourceOpen"
                    >
                      <div class="tracker-multi-select-tags">
                        <span v-for="val in form.trackerSource" :key="val" class="tracker-tag">
                          {{ getTrackerLabel(val) }}
                          <X
                            :size="12"
                            class="tracker-tag-remove"
                            @click.stop="removeTrackerSource(val)"
                          />
                        </span>
                        <span
                          v-if="form.trackerSource.length === 0"
                          class="tracker-multi-select-placeholder"
                          >Select sources...</span
                        >
                      </div>
                      <ChevronDown :size="16" class="tracker-multi-select-chevron" />
                    </button>
                  </PopoverTrigger>
                  <PopoverContent class="tracker-source-popover" align="start" :side-offset="4">
                    <div class="tracker-source-list">
                      <template v-for="group in trackerSourceOptions" :key="group.label">
                        <div class="tracker-source-group-label">
                          {{ group.label }}
                        </div>
                        <div
                          v-for="item in group.options"
                          :key="item.value"
                          class="tracker-source-option"
                          :class="{
                            'is-selected': form.trackerSource.includes(item.value),
                          }"
                          @click="toggleTrackerSource(item.value)"
                        >
                          <span class="tracker-source-option-label">{{ item.label }}</span>
                          <span v-if="item.cdn" class="tracker-cdn-badge">CDN</span>
                          <Check
                            v-if="form.trackerSource.includes(item.value)"
                            :size="14"
                            class="tracker-source-check"
                          />
                        </div>
                      </template>
                    </div>
                  </PopoverContent>
                </Popover>
                <ui-tooltip :content="$t('preferences.sync-tracker-tips')">
                  <ui-button
                    variant="outline"
                    size="sm"
                    @click="syncTrackerFromSource"
                    class="sync-tracker-btn"
                  >
                    <RefreshCw :size="12" class="animate-spin" v-if="trackerSyncing" />
                    <RefreshCcw width="12" height="12" v-else />
                  </ui-button>
                </ui-tooltip>
              </div>
              <Textarea
                :rows="3"
                autocomplete="off"
                :placeholder="`${$t('preferences.bt-tracker-input-tips')}`"
                v-model="form.btTracker"
                style="margin-top: 10px; max-height: 5lh"
              />
              <div class="form-info" style="margin-top: 8px">
                {{ $t('preferences.bt-tracker-tips') }}
                <a
                  target="_blank"
                  href="https://github.com/ngosang/trackerslist"
                  rel="noopener noreferrer"
                >
                  ngosang/trackerslist
                  <ExternalLink :size="12" />
                </a>
                <a
                  target="_blank"
                  href="https://github.com/XIU2/TrackersListCollection"
                  rel="noopener noreferrer"
                >
                  XIU2/TrackersListCollection
                  <ExternalLink :size="12" />
                </a>
              </div>
            </div>
            <div class="settings-row" style="margin-top: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.auto-sync-tracker') }}
                </div>
                <div class="settings-row-description" v-if="form.lastSyncTrackerTime > 0">
                  {{ new Date(form.lastSyncTrackerTime).toLocaleString() }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.autoSyncTracker"
                  @change="(val) => setAdvancedBoolean('autoSyncTracker', val)"
                />
              </div>
            </div>

          </div>
        </div>

        <!-- eDonkey Server Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Server :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.ed2k-server') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <Textarea
              :rows="3"
              autocomplete="off"
              :placeholder="`${$t('preferences.ed2k-server-input-tips')}`"
              v-model="form.ed2kServer"
              style="max-height: 5lh"
            />
            <div class="form-info" style="margin-top: 8px">
              {{ $t('preferences.ed2k-server-tips') }}
            </div>
          </div>
        </div>

        <!-- FTP / SFTP Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><FileKey :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.ftp-sftp-settings') }}</h3>
              <p>{{ $t('preferences.ftp-sftp-settings-tips') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.ftp-username') }}</label>
                <Input placeholder="anonymous" v-model="form.ftpUser" />
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.ftp-password') }}</label>
                <Input type="password" placeholder="motrix@" v-model="form.ftpPasswd" />
              </div>
            </div>
            <div class="settings-select-group" style="margin-top: 10px">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.sftp-private-key') }}</label>
                <Input placeholder="~/.ssh/id_rsa" v-model="form.sftpPrivateKey" />
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.sftp-key-passphrase') }}</label>
                <Input type="password" :placeholder="$t('preferences.sftp-key-passphrase-tips')" v-model="form.sftpKeyPassphrase" />
              </div>
            </div>
          </div>
        </div>

        <!-- Saved Credentials Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><KeyRound :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.saved-credentials') }}</h3>
              <p>{{ $t('preferences.saved-credentials-description') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <mo-credential-manager />
          </div>
        </div>

        <!-- M3U8 Output Format Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><FileText :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.m3u8-output-format') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.m3u8-output-format-label') }}</label>
                <Select v-model="form.m3u8OutputFormat">
                  <SelectTrigger class="w-30">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="ts">.ts</SelectItem>
                    <SelectItem value="mp4">.mp4 (ffmpeg)</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>
          </div>
        </div>

        <!-- RPC Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Cable :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.rpc') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{
                  $t('preferences.rpc-listen-port')
                }}</label>
                <div class="mo-input-group">
                  <Input
                    :placeholder="String(rpcDefaultPort)"
                    :maxlength="8"
                    v-model="form.rpcListenPort"
                    @blur="onRpcListenPortBlur"
                  />
                  <span class="mo-input-append">
                    <i @click.prevent="onRpcPortDiceClick" style="cursor: pointer">
                      <Dices :size="12" />
                    </i>
                  </span>
                </div>
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.rpc-secret') }}</label>
                <div class="mo-input-group">
                  <Input
                    :type="hideRpcSecret ? 'password' : 'text'"
                    placeholder="RPC Secret"
                    :maxlength="64"
                    v-model="form.rpcSecret"
                  />
                  <span class="mo-input-append">
                    <i @click.prevent="onRpcSecretDiceClick" style="cursor: pointer">
                      <Dices :size="12" />
                    </i>
                  </span>
                </div>
              </div>
            </div>
            <div class="form-info" style="margin-top: 8px">
              <a
                target="_blank"
                href="https://github.com/agalwood/Motrix/wiki/RPC"
                rel="noopener noreferrer"
              >
                {{ $t('preferences.rpc-secret-tips') }}
                <ExternalLink :size="12" />
              </a>
            </div>
          </div>
        </div>

        <!-- Ports Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Network :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.port') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.bt-port') }}</label>
                <div class="mo-input-group">
                  <Input placeholder="BT Port" :maxlength="8" v-model="form.listenPort" />
                  <span class="mo-input-append">
                    <i @click.prevent="onBtPortDiceClick" style="cursor: pointer">
                      <Dices :size="12" />
                    </i>
                  </span>
                </div>
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.dht-port') }}</label>
                <div class="mo-input-group">
                  <Input placeholder="DHT Port" :maxlength="8" v-model="form.dhtListenPort" />
                  <span class="mo-input-append">
                    <i @click.prevent="onDhtPortDiceClick" style="cursor: pointer">
                      <Dices :size="12" />
                    </i>
                  </span>
                </div>
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.ed2k-port') }}</label>
                <div class="mo-input-group">
                  <Input placeholder="4662" :maxlength="8" v-model="form.ed2kPort" />
                  <span class="mo-input-append">
                    <i @click.prevent="onEd2kPortDiceClick" style="cursor: pointer">
                      <Dices :size="12" />
                    </i>
                  </span>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Protocol Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Link :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.download-protocol') }}</h3>
              <p>{{ $t('preferences.protocols-default-client') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.protocols-magnet') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.protocols.magnet"
                  @change="(val) => onProtocolsChange('magnet', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.protocols-thunder') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.protocols.thunder"
                  @change="(val) => onProtocolsChange('thunder', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.protocols-ed2k') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.protocols.ed2k"
                  @change="(val) => onProtocolsChange('ed2k', val)"
                />
              </div>
            </div>
          </div>
        </div>

        <!-- User Agent Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><UserCircle :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.user-agent') }}</h3>
              <p>{{ $t('preferences.mock-user-agent') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <Textarea
              :rows="2"
              autocomplete="off"
              placeholder="User-Agent"
              v-model="form.userAgent"
            />
            <div class="ua-group">
              <ui-button size="sm" variant="outline" @click="() => changeUA('aria2')"
                >Aria2</ui-button
              >
              <ui-button size="sm" variant="outline" @click="() => changeUA('transmission')"
                >Transmission</ui-button
              >
              <ui-button size="sm" variant="outline" @click="() => changeUA('chrome')"
                >Chrome</ui-button
              >
              <ui-button size="sm" variant="outline" @click="() => changeUA('firefox')"
                >Firefox</ui-button
              >
              <ui-button size="sm" variant="outline" @click="() => changeUA('du')">du</ui-button>
            </div>
          </div>
        </div>

        <!-- Developer Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Code :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.developer') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <!-- File Paths -->
            <div class="dev-paths-grid">
              <div class="dev-path-card">
                <div class="dev-path-card-header">
                  <Database :size="13" class="dev-path-card-icon" />
                  <span class="dev-path-card-label">{{
                    $t('preferences.download-session-path')
                  }}</span>
                </div>
                <div class="dev-path-card-body">
                  <div class="mo-input-group">
                    <Input disabled :model-value="sessionPath" class="dev-path-input" />
                    <span class="mo-input-append">
                      <mo-show-in-folder v-if="isRenderer" :path="sessionPath" />
                    </span>
                  </div>
                </div>
              </div>
              <div class="dev-path-card">
                <div class="dev-path-card-header">
                  <ScrollText :size="13" class="dev-path-card-icon" />
                  <span class="dev-path-card-label">{{ $t('preferences.app-log-path') }}</span>
                </div>
                <div class="dev-path-card-body">
                  <div class="mo-input-group">
                    <Input disabled :model-value="logPath" class="dev-path-input" />
                    <span class="mo-input-append">
                      <mo-show-in-folder v-if="isRenderer" :path="logPath" />
                    </span>
                  </div>
                </div>
              </div>
              <div class="dev-path-card">
                <div class="dev-path-card-header">
                  <Settings :size="13" class="dev-path-card-icon" />
                  <span class="dev-path-card-label">Log Level</span>
                </div>
                <div class="dev-path-card-body">
                  <Select v-model="form.logLevel" class="dev-log-level-select">
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="item in logLevels" :key="item" :value="item">
                        {{ item }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>
            </div>

            <!-- Danger Zone -->
            <div class="dev-danger-zone">
              <div class="dev-danger-zone-label">
                <AlertTriangle :size="13" />
                Danger Zone
              </div>
              <div class="dev-danger-zone-actions">
                <div class="dev-danger-action">
                  <div class="dev-danger-action-info">
                    <span class="dev-danger-action-title">{{
                      $t('preferences.session-reset')
                    }}</span>
                  </div>
                  <ui-button variant="outline" size="sm" @click="() => onSessionResetClick()">
                    {{ $t('preferences.session-reset') }}
                  </ui-button>
                </div>
                <div class="dev-danger-action dev-danger-action--destructive">
                  <div class="dev-danger-action-info">
                    <span class="dev-danger-action-title">{{
                      $t('preferences.factory-reset')
                    }}</span>
                  </div>
                  <ui-button variant="destructive" size="sm" @click="() => onFactoryResetClick()">
                    {{ $t('preferences.factory-reset') }}
                  </ui-button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </form>
      <div class="form-actions">
        <ui-button @click="resetForm('advancedForm')">
          {{ $t('preferences.discard') }}
        </ui-button>
        <ui-button variant="primary" @click="submitForm('advancedForm')">
          {{ $t('preferences.save') }}
        </ui-button>
      </div>
    </main>
  </div>
</template>

<script lang="ts">
import {
	DEFAULT_ED2K_SERVERS,
	EMPTY_STRING,
	ENGINE_RPC_HOST,
	ENGINE_RPC_PORT,
	LOG_LEVELS,
	PROXY_SCOPE_OPTIONS,
	TRACKER_SOURCE_OPTIONS,
} from "@shared/constants";
import userAgentMap from "@shared/ua";
import {
	buildRpcUrl,
	changedConfig,
	convertCommaToLine,
	convertLineToComma,
	diffConfig,
	generateRandomInt,
	parseBooleanConfig,
} from "@shared/utils";
import logger from "@shared/utils/logger";
import {
	convertTrackerDataToLine,
	reduceTrackerString,
} from "@shared/utils/tracker";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { cloneDeep, extend, isEmpty } from "lodash";
import {
	AlertTriangle,
	Cable,
	Check,
	ChevronDown,
	Code,
	Database,
	Dices,
	ExternalLink,
	FileKey,
	FileText,
	Globe,
	KeyRound,
	Link,
	Network,
	Radio,
	RefreshCcw,
	RefreshCw,
	ScrollText,
	Server,
	Settings,
	UserCircle,
	X,
} from "lucide-vue-next";
import randomize from "randomatic";
import ShowInFolder from "@/components/Native/ShowInFolder.vue";
import CredentialManager from "@/components/Preference/CredentialManager.vue";
import SubnavSwitcher from "@/components/Subnav/SubnavSwitcher.vue";
import UiButton from "@/components/ui/compat/UiButton.vue";
import UiTooltip from "@/components/ui/compat/UiTooltip.vue";
import { confirm } from "@/components/ui/confirm-dialog";
import { Input } from "@/components/ui/input";
import {
	Popover,
	PopoverContent,
	PopoverTrigger,
} from "@/components/ui/popover";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import is from "@/shims/platform";
import { usePreferenceStore } from "@/store/preference";
import { useTaskStore } from "@/store/task";

const initForm = (config) => {
	const {
		autoCheckUpdate,
		autoSyncTracker,
		btTracker,
		dhtListenPort,
		ed2KPort: ed2kPort,
		ed2KServer: ed2kServer,
		hideAppMenu,
		lastCheckUpdateTime,
		lastSyncTrackerTime,
		listenPort,
		logLevel,
		protocols,
		proxy,
		rpcListenPort,
		rpcSecret,
		trackerSource,
		userAgent,
	} = config;
	// Accept both lodash camelCase key (m3U8OutputFormat from backend)
	// and form key (m3u8OutputFormat from initForm output fed back via extend)
	const m3u8OutputFormat = config.m3U8OutputFormat ?? config.m3u8OutputFormat;
	const result = {
		autoCheckUpdate: parseBooleanConfig(autoCheckUpdate),
		autoSyncTracker: parseBooleanConfig(autoSyncTracker),
		btTracker: convertCommaToLine(btTracker),
		dhtListenPort,
		ed2kPort: ed2kPort || 4662,
		ed2kServer: convertCommaToLine(ed2kServer || DEFAULT_ED2K_SERVERS),
		ftpUser: config.ftpUser || "",
		ftpPasswd: config.ftpPasswd || "",
		sftpPrivateKey: config.sftpPrivateKey || "",
		sftpKeyPassphrase: config.sftpPrivateKeyPassphrase || "",
		hideAppMenu,
		lastCheckUpdateTime,
		lastSyncTrackerTime,
		listenPort,
		logLevel,
		m3u8OutputFormat: m3u8OutputFormat || "ts",
		proxy: cloneDeep(proxy) || {
			enable: false,
			server: "",
			bypass: "",
			scope: [],
		},
		protocols: {
			magnet: parseBooleanConfig(protocols?.magnet, true),
			thunder: parseBooleanConfig(protocols?.thunder, false),
			ed2k: parseBooleanConfig(protocols?.ed2k, true),
		},
		rpcListenPort,
		rpcSecret,
		trackerSource,
		userAgent,
	};
	return result;
};

export default {
	name: "mo-preference-advanced",
	components: {
		[UiButton.name]: UiButton,
		"ui-tooltip": UiTooltip,
		[SubnavSwitcher.name]: SubnavSwitcher,
		[ShowInFolder.name]: ShowInFolder,
		[CredentialManager.name]: CredentialManager,
		Input,
		Textarea,
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
		Popover,
		PopoverContent,
		PopoverTrigger,
		RefreshCw,
		Globe,
		Radio,
		Cable,
		Network,
		Link,
		UserCircle,
		Code,
		FileKey,
		FileText,
		KeyRound,
		Database,
		ScrollText,
		AlertTriangle,
		Settings,
		Server,
		X,
		ChevronDown,
		Check,
		RefreshCcw,
		Dices,
		ExternalLink,
	},
	data() {
		const preferenceStore = usePreferenceStore();
		const formOriginal = initForm(preferenceStore.config);
		const form = initForm(extend({}, formOriginal, changedConfig.advanced));

		return {
			form,
			formOriginal,
			hideRpcSecret: true,
			proxyScopeOptions: PROXY_SCOPE_OPTIONS,
			trackerSourceOptions: TRACKER_SOURCE_OPTIONS,
			trackerSourceOpen: false,
			trackerSyncing: false,
		};
	},
	computed: {
		isRenderer: () => is.renderer(),
		title() {
			return this.$t("preferences.advanced");
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
				{
					key: "lab",
					title: this.$t("preferences.lab"),
					route: "/preference/lab",
				},
			];
		},
		rpcDefaultPort() {
			return ENGINE_RPC_PORT;
		},
		logLevels() {
			return LOG_LEVELS;
		},
		logPath() {
			return usePreferenceStore().config.logPath;
		},
		sessionPath() {
			return usePreferenceStore().config.sessionPath;
		},
	},
	watch: {
		"form.rpcListenPort": "syncRpcUrlToClipboard",
		"form.rpcSecret": "syncRpcUrlToClipboard",
	},
	methods: {
		setAdvancedBoolean(key, enable) {
			this.form[key] = !!enable;
		},
		syncRpcUrlToClipboard() {
			const url = buildRpcUrl({
				host: ENGINE_RPC_HOST,
				port: this.form.rpcListenPort,
				secret: this.form.rpcSecret,
			});
			writeText(url).catch(() => {
				/* noop */
			});
		},
		getTrackerLabel(value) {
			for (const group of this.trackerSourceOptions) {
				for (const item of group.options) {
					if (item.value === value) {
						return item.label;
					}
				}
			}
			return value;
		},
		toggleTrackerSource(value) {
			const idx = this.form.trackerSource.indexOf(value);
			if (idx >= 0) {
				this.form.trackerSource.splice(idx, 1);
			} else {
				this.form.trackerSource.push(value);
			}
		},
		removeTrackerSource(value) {
			const idx = this.form.trackerSource.indexOf(value);
			if (idx >= 0) {
				this.form.trackerSource.splice(idx, 1);
			}
		},
		onCheckUpdateClick() {
			this.$msg.info(this.$t("app.checking-for-updates"));
			invoke("check_for_updates").catch(() => {
				this.$msg.error(this.$t("app.update-error-message"));
			});
			usePreferenceStore()
				.fetchPreference()
				.then((config) => {
					const { lastCheckUpdateTime } = config;
					this.form.lastCheckUpdateTime = lastCheckUpdateTime;
				});
		},
		syncTrackerFromSource() {
			this.trackerSyncing = true;
			const { trackerSource } = this.form;
			usePreferenceStore()
				.fetchBtTracker(trackerSource)
				.then((data) => {
					const tracker = convertTrackerDataToLine(data);
					this.form.lastSyncTrackerTime = Date.now();
					this.form.btTracker = tracker;
					this.trackerSyncing = false;
				})
				.catch((_) => {
					this.trackerSyncing = false;
				});
		},
		onProtocolsChange(protocol, enabled) {
			const { protocols } = this.form;
			this.form.protocols = {
				...protocols,
				[protocol]: !!enabled,
			};
		},
		onProxyEnableChange(enable) {
			this.form.proxy = {
				...this.form.proxy,
				enable: !!enable,
			};
		},
		onProxyServerChange(server) {
			this.form.proxy = {
				...this.form.proxy,
				server,
			};
		},
		handleProxyBypassChange(bypass) {
			this.form.proxy = {
				...this.form.proxy,
				bypass: convertLineToComma(bypass),
			};
		},
		onProxyScopeChange(scope) {
			this.form.proxy = {
				...this.form.proxy,
				scope: [...scope],
			};
		},
		onProxyScopeToggle(item, checked) {
			const isChecked = !!checked;
			const scope = [...this.form.proxy.scope];
			const idx = scope.indexOf(item);
			if (isChecked && idx < 0) {
				scope.push(item);
			} else if (!isChecked && idx >= 0) {
				scope.splice(idx, 1);
			}
			this.form.proxy = {
				...this.form.proxy,
				scope,
			};
		},
		changeUA(type) {
			const ua = userAgentMap[type];
			if (!ua) {
				return;
			}
			this.form.userAgent = ua;
		},
		onBtPortDiceClick() {
			const port = generateRandomInt(20000, 24999);
			this.form.listenPort = port;
		},
		onDhtPortDiceClick() {
			const port = generateRandomInt(25000, 29999);
			this.form.dhtListenPort = port;
		},
		onEd2kPortDiceClick() {
			const port = generateRandomInt(30000, 34999);
			this.form.ed2kPort = port;
		},
		onRpcListenPortBlur() {
			if (
				EMPTY_STRING === this.form.rpcListenPort ||
				!this.form.rpcListenPort
			) {
				this.form.rpcListenPort = this.rpcDefaultPort;
			}
		},
		onRpcPortDiceClick() {
			const port = generateRandomInt(ENGINE_RPC_PORT, 20000);
			this.form.rpcListenPort = port;
		},
		onRpcSecretDiceClick() {
			this.hideRpcSecret = false;
			const rpcSecret = randomize("Aa0", 16);
			this.form.rpcSecret = rpcSecret;

			setTimeout(() => {
				this.hideRpcSecret = true;
			}, 2000);
		},
		async onSessionResetClick() {
			const { confirmed } = await confirm({
				message: this.$t("preferences.session-reset-confirm"),
				title: this.$t("preferences.session-reset"),
				kind: "warning",
				confirmText: this.$t("app.yes"),
				cancelText: this.$t("app.no"),
			});
			if (confirmed) {
				const taskStore = useTaskStore();
				taskStore.purgeTaskRecord();
				taskStore.pauseAllTask().then(() => {
					invoke("reset_session").catch(() => {
						/* noop */
					});
				});
			}
		},
		async onFactoryResetClick() {
			const { confirmed } = await confirm({
				message: this.$t("preferences.factory-reset-confirm"),
				title: this.$t("preferences.factory-reset"),
				kind: "warning",
				confirmText: this.$t("app.yes"),
				cancelText: this.$t("app.no"),
			});
			if (confirmed) {
				invoke("factory_reset").catch(() => {
					/* noop */
				});
			}
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
				...changedConfig.basic,
			};
			const booleanKeys = ["autoCheckUpdate", "autoSyncTracker"];
			for (const key of booleanKeys) {
				if (key in data) {
					data[key] = !!this.form[key];
				}
			}
			if ("protocols" in data) {
				data.protocols = {
					magnet: !!this.form.protocols.magnet,
					thunder: !!this.form.protocols.thunder,
					ed2k: !!this.form.protocols.ed2k,
				};
			}

			const { autoHideWindow, btTracker, ed2kServer, rpcListenPort } = data;

			// Remap form key to config key
			if ("sftpKeyPassphrase" in data) {
				data.sftpPrivateKeyPassphrase = data.sftpKeyPassphrase;
				delete data.sftpKeyPassphrase;
			}

			if (btTracker) {
				data.btTracker = reduceTrackerString(convertLineToComma(btTracker));
			}

			if (ed2kServer !== undefined) {
				data.ed2kServer = convertLineToComma(ed2kServer);
			}

			if (rpcListenPort === EMPTY_STRING) {
				data.rpcListenPort = this.rpcDefaultPort;
			}

			logger.log("[Motrix] preference changed data:", data);

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
								hidden: !!this.form.hideAppMenu,
							}).catch(() => {
								/* noop */
							});
						}
					}
					changedConfig.basic = {};
					changedConfig.advanced = {};
				})
				.catch((_e) => {
					this.$msg.error(this.$t("preferences.save-fail-message"));
				});
		},
		resetForm(_formName) {
			this.syncFormConfig();
		},
	},
	async beforeRouteLeave(to, _from) {
		changedConfig.advanced = diffConfig(this.formOriginal, this.form);
		if (to.path === "/preference/basic") {
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
