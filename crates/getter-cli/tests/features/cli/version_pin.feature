@getter-cli @version
Feature: Version pin state
  Scenario: User pins and unpins a package version baseline
    Given an initialized getter data directory
    When I run getter version pin for package "android/org.fdroid.fdroid" version "1.2.3"
    Then the command succeeds
    And the output is valid JSON
    And the pinned package version is "1.2.3"
    When I run getter version unpin for package "android/org.fdroid.fdroid"
    Then the command succeeds
    And the output is valid JSON
    And the package is unpinned
