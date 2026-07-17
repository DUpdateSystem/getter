Feature: Fresh-install package setup
  Scenario: User tracks an installed app through the unified setup flow
    Given an initialized getter data directory
    And an installed inventory with Android app "com.example.fallback" labeled "Fallback App"
    When I run getter startup for that inventory
    Then the command succeeds
    And setup state is "needs_package_setup"
    When I run getter setup preview for that inventory
    Then the command succeeds
    And I save the setup preview envelope to a file
    When I run getter setup apply for that preview with accept-all
    Then the command succeeds
    When I run getter startup for that inventory
    Then the command succeeds
    And setup state is "ready"
