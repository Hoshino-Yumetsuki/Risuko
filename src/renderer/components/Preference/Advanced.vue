<template>
  <div class="content panel panel-layout panel-layout--v">
    <mo-enter tag="header" preset="fadeInDown" class="panel-header">
      <h4 class="hidden-xs-only">{{ title }}</h4>
      <mo-subnav-switcher :title="title" :subnavs="subnavs" class="hidden-sm-and-up" />
    </mo-enter>
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

        <!-- Completion Script Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Terminal :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.completion-script') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.completion-script-enabled') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.completion-script-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.completionScriptEnabled"
                  @change="(val) => setAdvancedBoolean('completionScriptEnabled', val)"
                />
              </div>
            </div>
            <div v-if="form.completionScriptEnabled" style="margin-top: 14px">
              <div class="form-item-sub" style="margin-bottom: 10px">
                <label class="settings-select-item-label">{{
                  $t('preferences.completion-script-command')
                }}</label>
                <Input
                  v-model="form.completionScriptCommand"
                  :placeholder="
                    $t('preferences.completion-script-command-placeholder')
                  "
                />
              </div>
              <div class="form-item-sub" style="margin-bottom: 10px">
                <label class="settings-select-item-label">{{
                  $t('preferences.completion-script-args')
                }}</label>
                <Input
                  v-model="form.completionScriptArgs"
                  :placeholder="
                    $t('preferences.completion-script-args-placeholder')
                  "
                />
                <div class="form-info" style="margin-top: 6px">
                  {{ $t('preferences.completion-script-args-tips') }}
                </div>
              </div>
              <div class="form-item-sub" style="margin-bottom: 10px">
                <label class="settings-select-item-label">{{
                  $t('preferences.completion-script-timeout')
                }}</label>
                <NumberInput
                  v-model="form.completionScriptTimeoutMs"
                  :min="1000"
                  :max="300000"
                  :step="1000"
                />
              </div>
              <div class="form-item-sub">
                <ui-button
                  variant="outline"
                  size="sm"
                  :disabled="completionScriptTesting"
                  @click="onTestCompletionScript"
                >
                  {{ $t('preferences.completion-script-test') }}
                </ui-button>
                <div
                  v-if="completionScriptTestResult"
                  class="form-info"
                  style="margin-top: 8px; white-space: pre-wrap"
                >
                  {{ completionScriptTestResult }}
                </div>
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
                    href="https://github.com/YueMiyuki/Risuko/wiki/Proxy"
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

            <div class="settings-row" style="margin-top: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.bt-max-peers-per-torrent') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.bt-max-peers-per-torrent-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <NumberInput
                  v-model="form.btMaxPeersPerTorrent"
                  :min="10"
                  :max="500"
                  :step="1"
                />
              </div>
            </div>

            <div class="settings-row" style="margin-top: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.bt-max-outstanding-per-peer') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.bt-max-outstanding-per-peer-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <NumberInput
                  v-model="form.btMaxOutstandingPerPeer"
                  :min="8"
                  :max="512"
                  :step="1"
                />
              </div>
            </div>

            <div class="settings-row" style="margin-top: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.bt-enable-upnp') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.bt-enable-upnp-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.btEnableUpnp"
                  @change="(val) => setAdvancedBoolean('btEnableUpnp', val)"
                />
              </div>
            </div>

            <div class="settings-row" style="margin-top: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.bt-upnp-lease') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.bt-upnp-lease-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <NumberInput
                  v-model="form.btUpnpLease"
                  :min="60"
                  :max="86400"
                  :step="1"
                />
              </div>
            </div>

            <div class="settings-row" style="margin-top: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.bt-enable-lsd') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.bt-enable-lsd-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.btEnableLsd"
                  @change="(val) => setAdvancedBoolean('btEnableLsd', val)"
                />
              </div>
            </div>

            <div class="settings-row" style="margin-top: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.bt-encryption-policy') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.bt-encryption-policy-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <Select v-model="form.btEncryptionPolicy">
                  <SelectTrigger style="width: 180px">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="plaintext">
                      {{ $t('preferences.bt-encryption-plaintext') }}
                    </SelectItem>
                    <SelectItem value="prefer">
                      {{ $t('preferences.bt-encryption-prefer') }}
                    </SelectItem>
                    <SelectItem value="require">
                      {{ $t('preferences.bt-encryption-require') }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>

            <div class="settings-row" style="margin-top: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.bt-listen-v6') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.bt-listen-v6-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.btListenV6"
                  @change="(val) => setAdvancedBoolean('btListenV6', val)"
                />
              </div>
            </div>

          </div>
        </div>

        <!-- HTTP Network Reliability Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Globe :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.http-reliability') }}</h3>
              <p>{{ $t('preferences.http-reliability-tips') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group settings-select-group--stack">
              <div class="settings-select-item">
                <label class="settings-select-item-label">
                  {{ $t('preferences.connect-timeout') }} ({{ $t('preferences.unit-seconds') }})
                </label>
                <NumberInput v-model="form.connectTimeout" :min="1" :max="600" :step="1" />
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">
                  {{ $t('preferences.lowest-speed-limit') }} ({{ $t('preferences.unit-kib-per-sec') }})
                </label>
                <NumberInput v-model="form.lowestSpeedLimit" :min="0" :max="1048576" :step="1" />
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">
                  {{ $t('preferences.lowest-speed-limit-timeout') }} ({{ $t('preferences.unit-seconds') }})
                </label>
                <NumberInput v-model="form.lowestSpeedLimitTimeout" :min="1" :max="3600" :step="1" />
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{
                  $t('preferences.uri-selector')
                }}</label>
                <Select v-model="form.uriSelector" class="settings-select-control">
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="feedback">
                      {{ $t('preferences.uri-selector-feedback') }}
                    </SelectItem>
                    <SelectItem value="inorder">
                      {{ $t('preferences.uri-selector-inorder') }}
                    </SelectItem>
                    <SelectItem value="adaptive">
                      {{ $t('preferences.uri-selector-adaptive') }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div class="form-info" style="margin-top: 8px">
              {{ $t('preferences.lowest-speed-limit-help') }}
            </div>
          </div>
        </div>

        <!-- Storage Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><FileText :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.storage') }}</h3>
              <p>{{ $t('preferences.storage-tips') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{
                  $t('preferences.file-allocation')
                }}</label>
                <Select v-model="form.fileAllocation" class="settings-select-control">
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="falloc">
                      {{ $t('preferences.file-allocation-falloc') }}
                    </SelectItem>
                    <SelectItem value="trunc">
                      {{ $t('preferences.file-allocation-trunc') }}
                    </SelectItem>
                    <SelectItem value="none">
                      {{ $t('preferences.file-allocation-none') }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div class="form-info" style="margin-top: 8px">
              {{ $t('preferences.file-allocation-tips') }}
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
                <Input type="password" placeholder="risuko@" v-model="form.ftpPasswd" />
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

        <!-- YouTube Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Video :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.youtube-settings') }}</h3>
              <p>{{ $t('preferences.youtube-settings-tips') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-select-group settings-select-group--stack">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.youtube-format') }}</label>
                <Input
                  :placeholder="$t('preferences.youtube-format-placeholder')"
                  v-model="form.youtubeFormat"
                />
              </div>
              <div class="form-info" style="margin-top: 4px">
                {{ $t('preferences.youtube-format-tips') }}
              </div>
            </div>
          </div>
        </div>

        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Cable :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.rpc') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-row" style="margin-bottom: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">{{ $t('preferences.external-engine-enable') }}</div>
                <div class="settings-row-description">
                  {{ $t('preferences.external-engine-enable-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="form.externalEngineEnabled"
                  @change="(val) => setAdvancedBoolean('externalEngineEnabled', val)"
                />
              </div>
            </div>

            <div v-if="form.externalEngineEnabled" class="settings-select-group" style="margin-bottom: 8px">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{
                  $t('preferences.external-engine-ip')
                }}</label>
                <Input
                  :placeholder="externalRpcDefaultHost"
                  v-model="form.externalEngineHost"
                  @blur="onExternalEngineHostBlur"
                />
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{
                  $t('preferences.external-engine-port')
                }}</label>
                <Input
                  :placeholder="String(rpcDefaultPort)"
                  :maxlength="8"
                  v-model="form.externalEnginePort"
                  @blur="onExternalEnginePortBlur"
                />
              </div>
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.external-engine-secret') }}</label>
                <Input
                  :type="hideExternalRpcSecret ? 'password' : 'text'"
                  placeholder="RPC Secret"
                  :maxlength="64"
                  v-model="form.externalEngineSecret"
                />
              </div>
            </div>

            <div class="settings-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{
                  $t('preferences.rpc-listen-port')
                }}</label>
                <div class="mo-input-group">
                  <Input
                    :placeholder="String(rpcDefaultPort)"
                    :maxlength="8"
                    :disabled="form.externalEngineEnabled"
                    v-model="form.rpcListenPort"
                    @blur="onRpcListenPortBlur"
                  />
                  <span class="mo-input-append" v-if="!form.externalEngineEnabled">
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
                    :disabled="form.externalEngineEnabled"
                    v-model="form.rpcSecret"
                  />
                  <span class="mo-input-append" v-if="!form.externalEngineEnabled">
                    <i @click.prevent="onRpcSecretDiceClick" style="cursor: pointer">
                      <Dices :size="12" />
                    </i>
                  </span>
                </div>
              </div>
            </div>
            <div class="form-info" style="margin-top: 8px; display: flex; align-items: center; gap: 8px">
              <a
                target="_blank"
                href="https://github.com/YueMiyuki/Risuko/wiki/RPC"
                rel="noopener noreferrer"
              >
                {{ $t('preferences.rpc-secret-tips') }}
                <ExternalLink :size="12" />
              </a>
              <ui-button size="sm" variant="outline" @click="copyRpcUrlToClipboard">
                <Copy :size="12" />
                {{ $t('preferences.copy-rpc-url') }}
              </ui-button>
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
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.protocols-adc') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.protocols.adc"
                  @change="(val) => onProtocolsChange('adc', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.protocols-gnutella') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.protocols.gnutella"
                  @change="(val) => onProtocolsChange('gnutella', val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.protocols-g2') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.protocols.g2"
                  @change="(val) => onProtocolsChange('g2', val)"
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

        <!-- Cookies Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Cookie :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.cookies') }}</h3>
              <p>{{ $t('preferences.cookies-tips') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <Input
              v-model="form.loadCookies"
              :placeholder="$t('preferences.load-cookies-placeholder')"
              autocomplete="off"
            />
          </div>
        </div>

        <!-- Netrc auth Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><KeyRound :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.netrc') }}</h3>
              <p>{{ $t('preferences.netrc-tips') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <Input
              v-model="form.netrcPath"
              :placeholder="$t('preferences.netrc-path-placeholder')"
              autocomplete="off"
            />
            <div class="settings-row" style="margin-top: 8px">
              <div class="settings-row-content">
                <div class="settings-row-title">
                  {{ $t('preferences.no-netrc') }}
                </div>
                <div class="settings-row-description">
                  {{ $t('preferences.no-netrc-tips') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!form.noNetrc"
                  @change="(val) => setAdvancedBoolean('noNetrc', val)"
                />
              </div>
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

            <div class="settings-select-group settings-select-group--stack" style="margin-top: 12px">
              <div class="settings-select-item">
                <label class="settings-select-item-label">{{ $t('preferences.engine-overrides') }}</label>
                <Textarea
                  :rows="8"
                  autocomplete="off"
                  :placeholder="$t('preferences.engine-overrides-placeholder')"
                  v-model="form.engineOverridesText"
                  style="max-height: 16lh"
                />
              </div>
              <div class="form-info" style="margin-top: 4px">
                {{ $t('preferences.engine-overrides-tips') }}
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
	Cookie,
	Copy,
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
	Terminal,
	UserCircle,
	Video,
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
import NumberInput from "@/components/ui/NumberInput.vue";
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
		engineOverridesText,
		engineOverrides,
		externalEngineEnabled,
		externalEngineHost,
		externalEnginePort,
		externalEngineSecret,
		btTracker,
		btMaxPeersPerTorrent,
		btMaxOutstandingPerPeer,
		btEnableUpnp,
		btUpnpLease,
		btEnableLsd,
		btEncryptionPolicy,
		btListenV6,
		connectTimeout,
		lowestSpeedLimit,
		lowestSpeedLimitTimeout,
		fileAllocation,
		uriSelector,
		loadCookies,
		netrcPath,
		noNetrc,
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
		completionScriptEnabled,
		completionScriptCommand,
		completionScriptArgs,
		completionScriptTimeoutMs,
	} = config;
	// Accept both lodash camelCase key (m3U8OutputFormat from backend)
	// and form key (m3u8OutputFormat from initForm output fed back via extend)
	const m3u8OutputFormat = config.m3U8OutputFormat ?? config.m3u8OutputFormat;
	const pendingEngineOverridesText =
		typeof engineOverridesText === "string"
			? engineOverridesText
			: JSON.stringify(engineOverrides || {}, null, 2);
	const result = {
		autoCheckUpdate: parseBooleanConfig(autoCheckUpdate),
		autoSyncTracker: parseBooleanConfig(autoSyncTracker),
		engineOverridesText: pendingEngineOverridesText,
		externalEngineEnabled: parseBooleanConfig(externalEngineEnabled, false),
		externalEngineHost: externalEngineHost || ENGINE_RPC_HOST,
		externalEnginePort: externalEnginePort || ENGINE_RPC_PORT,
		externalEngineSecret: externalEngineSecret || "",
		btTracker: convertCommaToLine(btTracker),
		btMaxPeersPerTorrent: btMaxPeersPerTorrent ?? 100,
		btMaxOutstandingPerPeer: btMaxOutstandingPerPeer ?? 128,
		btEnableUpnp: parseBooleanConfig(btEnableUpnp, true),
		btUpnpLease: btUpnpLease ?? 300,
		btEnableLsd: parseBooleanConfig(btEnableLsd, true),
		btEncryptionPolicy: btEncryptionPolicy || "prefer",
		btListenV6: parseBooleanConfig(btListenV6, false),
		connectTimeout: connectTimeout ?? 60,
		// `lowestSpeedLimit` is stored as bytes/s by the engine but the UI
		// buckets it into KiB/s for readability; the save handler multiplies
		// back by 1024. As a side effect, backend values that are not exact
		// multiples of 1024 get rounded to the nearest KiB whenever the user
		// edits this field. That precision loss is intentional — it keeps the
		// displayed number consistent with the UI unit
		lowestSpeedLimit: Math.round((Number(lowestSpeedLimit) || 0) / 1024),
		lowestSpeedLimitTimeout: lowestSpeedLimitTimeout ?? 30,
		fileAllocation: fileAllocation || "falloc",
		uriSelector: uriSelector || "feedback",
		loadCookies: loadCookies || "",
		netrcPath: netrcPath || "",
		noNetrc: parseBooleanConfig(noNetrc, false),
		dhtListenPort,
		ed2kPort: ed2kPort || 4662,
		ed2kServer: convertCommaToLine(ed2kServer || DEFAULT_ED2K_SERVERS),
		ftpUser: config.ftpUser || "",
		ftpPasswd: config.ftpPasswd || "",
		sftpPrivateKey: config.sftpPrivateKey || "",
		sftpKeyPassphrase: config.sftpPrivateKeyPassphrase || "",
		youtubeFormat: config.youtubeFormat ?? config["youtube-format"] ?? "",
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
			adc: parseBooleanConfig(protocols?.adc, false),
			gnutella: parseBooleanConfig(protocols?.gnutella, false),
			g2: parseBooleanConfig(protocols?.g2, false),
		},
		rpcListenPort,
		rpcSecret,
		trackerSource,
		userAgent,
		completionScriptEnabled: parseBooleanConfig(completionScriptEnabled, false),
		completionScriptCommand: completionScriptCommand || "",
		completionScriptArgs: completionScriptArgs || "",
		completionScriptTimeoutMs: completionScriptTimeoutMs ?? 30000,
	};
	return result;
};

const sanitizeEngineOverrides = (input) => {
	const reservedKeys = new Set(["rpc-host"]);
	const filtered = {};
	const droppedKeys = [];
	for (const [key, value] of Object.entries(input || {})) {
		const normalized = `${key}`.toLowerCase();
		if (
			reservedKeys.has(normalized) ||
			normalized.startsWith("rpc-") ||
			normalized.endsWith("-port") ||
			normalized.endsWith("-secret")
		) {
			droppedKeys.push(key);
			continue;
		}
		filtered[key] = value;
	}
	return { filtered, droppedKeys };
};

const normalizePortValue = (value, fallback) => {
	const normalized = `${value ?? ""}`.trim();
	if (!normalized) {
		return fallback;
	}

	if (!/^\d+$/.test(normalized)) {
		return fallback;
	}

	const parsed = Number.parseInt(normalized, 10);
	if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
		return fallback;
	}

	return parsed;
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
		NumberInput,
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
		Cookie,
		FileKey,
		FileText,
		KeyRound,
		ScrollText,
		AlertTriangle,
		Settings,
		Server,
		Terminal,
		X,
		ChevronDown,
		Check,
		RefreshCcw,
		Dices,
		ExternalLink,
		Copy,
		Video,
	},
	data() {
		const preferenceStore = usePreferenceStore();
		const formOriginal = initForm(preferenceStore.config);
		const form = initForm(extend({}, formOriginal, changedConfig.advanced));

		return {
			form,
			formOriginal,
			hideRpcSecret: true,
			hideExternalRpcSecret: true,
			proxyScopeOptions: PROXY_SCOPE_OPTIONS,
			trackerSourceOptions: TRACKER_SOURCE_OPTIONS,
			trackerSourceOpen: false,
			trackerSyncing: false,
			completionScriptTesting: false,
			completionScriptTestResult: "",
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
			];
		},
		rpcDefaultPort() {
			return ENGINE_RPC_PORT;
		},
		externalRpcDefaultHost() {
			return ENGINE_RPC_HOST;
		},
		logLevels() {
			return LOG_LEVELS;
		},
		logPath() {
			return usePreferenceStore().config.appLogPath;
		},
		currentRpcUrl() {
			const host = this.form.externalEngineEnabled
				? this.form.externalEngineHost
				: ENGINE_RPC_HOST;
			const port = this.form.externalEngineEnabled
				? this.form.externalEnginePort
				: this.form.rpcListenPort;
			const secret = this.form.externalEngineEnabled
				? this.form.externalEngineSecret
				: this.form.rpcSecret;
			return buildRpcUrl({ host, port, secret });
		},
	},
	methods: {
		setAdvancedBoolean(key, enable) {
			this.form[key] = !!enable;
		},
		copyRpcUrlToClipboard() {
			writeText(this.currentRpcUrl).catch(() => {
				/* noop */
			});
		},
		onExternalEngineHostBlur() {
			const host = `${this.form.externalEngineHost || ""}`.trim();
			this.form.externalEngineHost = host || ENGINE_RPC_HOST;
		},
		onExternalEnginePortBlur() {
			this.form.externalEnginePort = normalizePortValue(
				this.form.externalEnginePort,
				this.rpcDefaultPort,
			);
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
		async onTestCompletionScript() {
			const command = `${this.form.completionScriptCommand || ""}`.trim();
			if (!command) {
				this.$msg.error(this.$t("preferences.completion-script-empty"));
				return;
			}
			this.completionScriptTesting = true;
			this.completionScriptTestResult = "";
			try {
				const result = await invoke<{
					success: boolean;
					exit_code: number | null;
					stdout: string;
					stderr: string;
					duration_ms: number;
					timed_out: boolean;
					message: string | null;
				}>("test_completion_script", {
					command,
					args: this.form.completionScriptArgs || "",
					timeoutMs: Number(this.form.completionScriptTimeoutMs) || 30000,
				});
				const lines = [
					`exit=${result.exit_code ?? "n/a"} duration=${result.duration_ms}ms timed_out=${result.timed_out}`,
				];
				if (result.message) {
					lines.push(`message: ${result.message}`);
				}
				if (result.stdout) {
					lines.push(`stdout:\n${result.stdout}`);
				}
				if (result.stderr) {
					lines.push(`stderr:\n${result.stderr}`);
				}
				this.completionScriptTestResult = lines.join("\n");
				if (result.success) {
					this.$msg.success(this.$t("preferences.completion-script-test-ok"));
				} else {
					this.$msg.error(this.$t("preferences.completion-script-test-fail"));
				}
			} catch (err: unknown) {
				this.completionScriptTestResult = `${(err as Error)?.message || err}`;
				this.$msg.error(this.$t("preferences.completion-script-test-fail"));
			} finally {
				this.completionScriptTesting = false;
			}
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

			if ("engineOverridesText" in data) {
				const raw = `${this.form.engineOverridesText || ""}`.trim();
				if (!raw) {
					data.engineOverrides = {};
				} else {
					try {
						const parsed = JSON.parse(raw);
						if (
							!parsed ||
							typeof parsed !== "object" ||
							Array.isArray(parsed)
						) {
							this.$msg.error(this.$t("preferences.engine-overrides-invalid"));
							return;
						}
						const { filtered, droppedKeys } = sanitizeEngineOverrides(parsed);
						data.engineOverrides = filtered;
						if (droppedKeys.length) {
							this.$msg.warning(
								this.$t("preferences.engine-overrides-reserved-keys", {
									keys: droppedKeys.join(", "),
								}),
							);
						}
					} catch {
						this.$msg.error(this.$t("preferences.engine-overrides-invalid"));
						return;
					}
				}
				delete data.engineOverridesText;
			}
			const booleanKeys = [
				"autoCheckUpdate",
				"autoSyncTracker",
				"externalEngineEnabled",
				"completionScriptEnabled",
			];
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
					adc: !!this.form.protocols.adc,
					gnutella: !!this.form.protocols.gnutella,
					g2: !!this.form.protocols.g2,
				};
			}

			const {
				autoHideWindow,
				btTracker,
				ed2kServer,
				rpcListenPort,
				externalEnginePort,
			} = data;

			// Remap form key to config key
			if ("sftpKeyPassphrase" in data) {
				data.sftpPrivateKeyPassphrase = data.sftpKeyPassphrase;
				delete data.sftpKeyPassphrase;
			}

			// Remap YouTube form keys to kebab-case config keys
			if ("youtubeFormat" in data) {
				data["youtube-format"] = data.youtubeFormat;
				delete data.youtubeFormat;
			}

			if (btTracker) {
				data.btTracker = reduceTrackerString(convertLineToComma(btTracker));
			}

			for (const key of [
				"btMaxPeersPerTorrent",
				"btMaxOutstandingPerPeer",
				"btUpnpLease",
				"connectTimeout",
				"lowestSpeedLimit",
				"lowestSpeedLimitTimeout",
			]) {
				if (key in data) {
					const raw = data[key];
					if (raw === "" || raw === null || raw === undefined) {
						// Blank input means "use engine default"; drop the key
						// instead of coercing to 0 so the stored value matches
						delete data[key];
						continue;
					}
					const n = Number(raw);
					// Per-key bounds mirror the `:min`/`:max` constraints set on the
					// matching `<NumberInput>` in the form. Enforcing both bounds
					// here protects against programmatic input (e.g. config import,
					// IPC) that bypasses the spinner clamping
					let ok: boolean;
					if (key === "btUpnpLease") {
						ok = Number.isFinite(n) && n >= 60 && n <= 86400;
					} else if (key === "connectTimeout") {
						ok = Number.isFinite(n) && n >= 1 && n <= 600;
					} else if (key === "lowestSpeedLimitTimeout") {
						ok = Number.isFinite(n) && n >= 1 && n <= 3600;
					} else if (key === "lowestSpeedLimit") {
						// UI value is KiB/s (max 1 GiB/s); save handler converts to
						// bytes/s afterwards
						ok = Number.isFinite(n) && n >= 0 && n <= 1048576;
					} else {
						ok = Number.isFinite(n) && n >= 0;
					}
					if (ok) {
						data[key] = n;
					} else {
						delete data[key];
					}
				}
			}

			// `lowest-speed-limit` is shown to users in KiB/s but the engine
			// expects bytes/s. Convert here so the stored config matches the
			// UI unit while the engine sees the correct magnitude
			if (
				"lowestSpeedLimit" in data &&
				typeof data.lowestSpeedLimit === "number"
			) {
				data.lowestSpeedLimit = data.lowestSpeedLimit * 1024;
			}

			if (ed2kServer !== undefined) {
				data.ed2kServer = convertLineToComma(ed2kServer);
			}

			if (rpcListenPort === EMPTY_STRING) {
				data.rpcListenPort = this.rpcDefaultPort;
			}

			if (externalEnginePort !== undefined) {
				data.externalEnginePort = normalizePortValue(
					this.form.externalEnginePort,
					this.rpcDefaultPort,
				);
			}

			if ("externalEngineHost" in data) {
				const host = `${this.form.externalEngineHost || ""}`.trim();
				data.externalEngineHost = host || ENGINE_RPC_HOST;
			}

			{
				const safeData = { ...data };
				if ("externalEngineSecret" in safeData) {
					safeData.externalEngineSecret = "[REDACTED]";
				}
				if (
					safeData.engineOverrides &&
					typeof safeData.engineOverrides === "object"
				) {
					safeData.engineOverrides = Object.fromEntries(
						Object.keys(safeData.engineOverrides).map((k) => [k, "[REDACTED]"]),
					);
				}
				logger.log("[Risuko] preference changed data:", safeData);
			}

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
