# Android recipe

This page shows how to build an Android app in a Task and run it on an
emulator in the Box. The worked example is `doorlock-app`, a Flutter app with
a flavor named `dev`, an AVD named `doorlock`, and a backend that docker
compose starts on port 3004. Swap in your own names and commands where the
example uses the app's.

The Box image carries the whole toolchain, so setup installs nothing. Setup
starts only the backend.

## Tools the Box image must provide

The image must provide the tools in
[Box image requirements](../README.md#box-image-requirements), plus:

- A JDK with `keytool`. JDK 17 works. moor needs `keytool` to give Java and
  Gradle the Proxy settings.
- The Android SDK with its licences accepted: `platform-tools` (for `adb`),
  `emulator`, `cmdline-tools` (for `avdmanager`), and the `platforms` and
  `build-tools` versions the app targets.
- A system image for the emulator, such as
  `system-images;android-35;default;x86_64`.
- An AVD made from that system image. Create it under `$HOME/.android/avd`,
  for example in the image's entrypoint on first boot.
- Flutter, if the app uses it, with the Android artifacts precached.
- Whatever setup uses to start the backend. The example uses docker and
  docker compose.

Write the toolchain `PATH` to `/etc/environment` as well as to the
Dockerfile `ENV`. moor runs every command in the Box over ssh, and those
commands do not inherit the Dockerfile `ENV`. moor adds the `PATH` from
`/etc/environment` to each one. Without that line, setup cannot find
`flutter`.

The image does not need the NDK. The first build downloads the NDK version the
app asks for from `dl.google.com`.

## Repo config

```yaml
setup:
  - flutter --version
  - make dev-api
teardown:
  - adb emu kill || true
  - timeout 20 sh -c 'while adb devices | grep -q "^emulator-"; do sleep 2; done' || pkill -f '[q]emu-system' || true
  - make dev-api-down
box:
  size: medium
proxy:
  direct:
    - repo.maven.apache.org
    - dl.google.com
    - services.gradle.org
    - plugins.gradle.org
    - plugins-artifacts.gradle.org
    - pub.dev
    - storage.googleapis.com
    - release-assets.githubusercontent.com
```

`flutter --version` fails setup early if the toolchain is not on `PATH`.
`make dev-api` starts the backend and waits until it answers on port 3004.
Replace both `make` targets with your own commands to start and stop the
backend.

`adb emu kill` can return before the emulator stops, and sometimes it does
not stop the emulator at all. The second teardown line waits up to 20 seconds
for `adb devices` to list no emulator. If one is still listed, the line kills
the emulator process. The brackets in `'[q]emu-system'` stop the pattern from
matching the teardown script's own command line, which contains the pattern
text.

### Hosts that go direct

Each host in `proxy.direct` serves public downloads that need no credentials,
so the Box fetches them without the Proxy. A cold build moves hundreds of
megabytes from these hosts. See [Direct hosts](../README.md#direct-hosts) for
how moor applies the list.

| Host | What the build fetches from it |
|---|---|
| `repo.maven.apache.org` | Maven Central dependencies |
| `dl.google.com` | Google's Maven repository (the Android Gradle plugin and AndroidX) and SDK packages such as the NDK |
| `services.gradle.org` | The Gradle distribution that the wrapper asks for |
| `plugins.gradle.org` | The Gradle Plugin Portal |
| `plugins-artifacts.gradle.org` | The Gradle Plugin Portal's artifact files |
| `pub.dev` | Dart and Flutter package metadata |
| `storage.googleapis.com` | Dart package archives and Flutter engine artifacts |
| `release-assets.githubusercontent.com` | The Gradle distribution file, after `services.gradle.org` redirects through `github.com` |

`storage.googleapis.com` serves every public Cloud Storage bucket, so the Box
reaches all of them without the Proxy.

`github.com` cannot go direct. It is a credential rule's host, and moor
refuses a `proxy.direct` entry that covers one. The wrapper's request to
`github.com` goes through the Proxy, and only the redirect target goes
direct.

## Java and Gradle through the Proxy

When a Task has a Proxy, moor points Java and Gradle at it for every host that
is not direct:

- moor adds a marked block to `~/.gradle/gradle.properties` (or
  `$GRADLE_USER_HOME/gradle.properties`) with the Proxy host, port, login,
  and `nonProxyHosts`. Your other lines in the file are kept. The file is mode
  600, because it holds the Proxy login.
- moor builds a trust store from the JDK certificates plus the Proxy CA, and
  points Gradle and `JAVA_TOOL_OPTIONS` at it.
- `JAVA_TOOL_OPTIONS` carries the Proxy address and trust store but never the
  login, because the JVM prints that variable.

If the Box has no `keytool`, moor sets none of this.

## Build the app, then start the emulator

Run these commands in the Box with `moor run`, or give them to the agent.
Start the emulator only after the build ends, so the two do not compete for
the Box's memory.

1. Build the debug APK:

   ```sh
   flutter build apk --debug --flavor dev
   ```

   The APK is `build/app/outputs/flutter-apk/app-dev-debug.apk`. For a plain
   Gradle project, run `./gradlew assembleDevDebug`.

2. Start the emulator without a window, then wait until Android has booted:

   ```sh
   (setsid nohup emulator -avd doorlock -no-window -no-audio -no-boot-anim \
     -accel on -gpu swiftshader_indirect > emulator.log 2>&1 &)
   adb wait-for-device
   until [ "$(adb shell getprop sys.boot_completed | tr -d '\r')" = 1 ]; do
     sleep 2
   done
   ```

   `setsid nohup` keeps the emulator running after the `moor run` command
   that started it returns.

3. Forward the emulator's port 3004 to the backend on the Box:

   ```sh
   adb reverse tcp:3004 tcp:3004
   ```

   The app keeps its default endpoint, `http://localhost:3004`. Inside the
   emulator, `localhost` is the emulator itself, so `adb reverse` sends the
   app's requests to the Box. The build command stays the same for every Box.

4. Install the APK and start the app by its applicationId:

   ```sh
   adb install -r -g build/app/outputs/flutter-apk/app-dev-debug.apk
   adb shell monkey -p com.oursky.opensky.dev -c android.intent.category.LAUNCHER 1
   ```

   Use the applicationId of the flavor you built. Read it from
   `applicationId` and the flavor's `applicationIdSuffix` in the app's Gradle
   file. Do not search `pm list packages` for the app's name. The example
   app's dev flavor is `com.oursky.opensky.dev`, which does not contain the
   repo name.

5. Check the installed version:

   ```sh
   adb shell dumpsys package com.oursky.opensky.dev | grep -E 'versionName|versionCode'
   ```

## Typical phase times

These times come from one run of the example on a Box with 4 CPUs and 8 GB of
memory. Use them as a reference for what to expect, not as limits.

| Phase | Time |
|---|---|
| Box request to Box ready | 13 s |
| Repo sync to the Box | 69 s |
| Setup (`flutter --version`, then `make dev-api`) | 78 s |
| First build, cold caches, NDK download included | 464 s |
| Emulator start to `sys.boot_completed` | 34 s |
| App launch to its login screen | 6 s |
| Second build by the agent, warm Gradle cache | 17 s |
| `moor close`, from command to Box deleted | 110 s |

The whole run used 28 VM minutes. About 5 of those minutes were idle time
between the commands that drove the run.

## Operator settings

The provider, the template, and the disk size belong to the operator, never to
the repo config. Pick the provider with `box.provider` in your moor user
config, as [Box provider](../README.md#box-provider) describes. Set the
template and the disk size in the crabbox user config. With the agent37
provider, they go in the provider's `config` section:

```yaml
external:
  config:
    template: moor-android@1
    diskGb: 20
```

`template` names the image that carries the toolchain above. `diskGb` sets the
durable disk, which is mounted at `/root`. That disk holds the Gradle cache,
the pub cache, the NDK, the AVD, and docker's data. 20 GB was enough for the
example app. The Box must also expose `/dev/kvm`, so the emulator can run
with `-accel on`. See
the [crabbox-agent37](https://github.com/IniZio/crabbox-agent37) README for
the rest of its config.
