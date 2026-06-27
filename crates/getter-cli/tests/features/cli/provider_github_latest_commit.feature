@getter-cli @provider @github
Feature: GitHub latest commit provider fixtures
  Scenario: User queries fixture-backed GitHub latest commit as a live revision
    Given an initialized getter data directory
    And a fixture GitHub commit response for "DUpdateSystem/UpgradeAll"
    When I run getter provider github latest-commit for owner "DUpdateSystem" repo "UpgradeAll"
    Then the command succeeds
    And the output is valid JSON
    And the GitHub latest-commit provider returns live revision "0123456789abcdef0123456789abcdef01234567"
    And the autogen repository has not been written
