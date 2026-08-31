@getter-cli @provider @github
Feature: GitHub release provider fixtures
  Scenario: User queries fixture-backed GitHub releases without writing generated packages
    Given an initialized getter data directory
    And a fixture GitHub releases response for "DUpdateSystem/UpgradeAll"
    When I run getter provider github releases for owner "DUpdateSystem" repo "UpgradeAll"
    Then the command succeeds
    And the output is valid JSON
    And the GitHub release provider returns candidate "v1.2.0" with artifact "app-release.apk"
    And the autogen repository has not been written
