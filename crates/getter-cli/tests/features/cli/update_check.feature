@getter-cli @update
Feature: Offline update check
  Scenario: User checks an offline fixture with an available update
    Given an initialized getter data directory
    And an offline update fixture for package "android/org.fdroid.fdroid" installed version "1.0.0" with candidate versions "1.0.1,1.2.0"
    When I run getter update check for that fixture
    Then the command succeeds
    And the output is valid JSON
    And the update check status is "update_available"
    And the selected update version is "1.2.0"
    And the update check actions download file "app.apk" and request installer "android_package"

  Scenario: User checks an offline fixture that is already up to date
    Given an initialized getter data directory
    And an offline update fixture for package "android/org.fdroid.fdroid" installed version "2.0.0" with candidate versions "1.9.0,2.0.0"
    When I run getter update check for that fixture
    Then the command succeeds
    And the output is valid JSON
    And the update check status is "up_to_date"
    And the update check has no selected update

  Scenario: User checks an offline fixture where pin_version overrides the local baseline
    Given an initialized getter data directory
    And an offline update fixture for package "android/org.fdroid.fdroid" installed version "1.0.0" pin version "1.2.0" with candidate versions "1.1.0,1.2.0,1.3.0"
    When I run getter update check for that fixture
    Then the command succeeds
    And the output is valid JSON
    And the update check status is "update_available"
    And the selected update version is "1.3.0"

  Scenario: User checks an offline fixture where pin_version makes the package up to date
    Given an initialized getter data directory
    And an offline update fixture for package "android/org.fdroid.fdroid" installed version "1.0.0" pin version "1.2.0" with candidate versions "1.2.0"
    When I run getter update check for that fixture
    Then the command succeeds
    And the output is valid JSON
    And the update check status is "up_to_date"
    And the update check has no selected update

  Scenario: User checks an offline fixture without an installed version
    Given an initialized getter data directory
    And an offline update fixture for package "android/org.fdroid.fdroid" without installed version with candidate versions "1.0.0-beta,1.0.0"
    When I run getter update check for that fixture
    Then the command succeeds
    And the output is valid JSON
    And the update check status is "update_available"
    And the selected update version is "1.0.0"

  Scenario: User checks an offline fixture with no candidates
    Given an initialized getter data directory
    And an offline update fixture for package "android/org.fdroid.fdroid" installed version "1.0.0" with candidate versions ""
    When I run getter update check for that fixture
    Then the command succeeds
    And the output is valid JSON
    And the update check status is "no_candidates"
    And the update check has no selected update

  Scenario: User receives structured errors when the selected update has no artifact
    Given an initialized getter data directory
    And an offline update fixture for package "android/org.fdroid.fdroid" installed version "1.0.0" with artifactless candidate version "1.2.0"
    When I run getter update check for that fixture
    Then the command fails with an update check error

  Scenario: User receives structured errors for malformed offline update fixtures
    Given an initialized getter data directory
    And a malformed offline update fixture
    When I run getter update check for that fixture
    Then the command fails with an update check error
