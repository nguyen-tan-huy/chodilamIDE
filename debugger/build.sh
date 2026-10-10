#!/usr/bin/env bash
# Builds build/chodilamide-debugger.jar (runs on JDK 17+, debugs JVMs 8+). No dependencies.
set -euo pipefail
cd "$(dirname "$0")"
JAVA_HOME="${JAVA_HOME:-$(ls -d /usr/lib/jvm/java-21-openjdk 2>/dev/null || true)}"
JAVAC="${JAVA_HOME:+$JAVA_HOME/bin/}javac"
JAR="${JAVA_HOME:+$JAVA_HOME/bin/}jar"
rm -rf build/classes && mkdir -p build/classes
"$JAVAC" --release 17 -encoding UTF-8 -Xlint:unchecked -d build/classes $(find src -name '*.java')
"$JAR" --create --file build/chodilamide-debugger.jar --main-class chodilamide.debugger.Main -C build/classes .
echo "built $(pwd)/build/chodilamide-debugger.jar"

# test runner: runs inside the target JVM (JDK 8+), compiled against the oldest APIs it uses
M2="${HOME}/.m2/repository"
CP="$M2/org/junit/platform/junit-platform-launcher/1.8.2/junit-platform-launcher-1.8.2.jar:$M2/org/junit/platform/junit-platform-engine/1.8.2/junit-platform-engine-1.8.2.jar:$M2/org/junit/platform/junit-platform-commons/1.8.2/junit-platform-commons-1.8.2.jar:$M2/org/opentest4j/opentest4j/1.2.0/opentest4j-1.2.0.jar:$M2/junit/junit/4.13.2/junit-4.13.2.jar"
rm -rf build/testrunner && mkdir -p build/testrunner
"$JAVAC" --release 8 -encoding UTF-8 -nowarn -cp "$CP" -d build/testrunner $(find testrunner/src -name '*.java')
"$JAR" --create --file build/chodilamide-testrunner.jar -C build/testrunner .
echo "built $(pwd)/build/chodilamide-testrunner.jar"
