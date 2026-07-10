package storage

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"time"
)

type Client struct {
	baseURL    string
	httpClient *http.Client
}

func NewClient(baseURL string) *Client {
	return &Client{
		baseURL:    baseURL,
		httpClient: &http.Client{Timeout: 10 * time.Second},
	}
}

type Skill struct {
	Name        string  `json:"name"`
	Description *string `json:"description,omitempty"`
	Prompt      string  `json:"prompt"`
	CreatedBy   *string `json:"created_by,omitempty"`
}

func (c *Client) LoadSkills() (map[string]Skill, error) {
	resp, err := c.httpClient.Get(c.baseURL + "/skills")
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	var v struct {
		Skills map[string]Skill `json:"skills"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&v); err != nil {
		return nil, err
	}
	return v.Skills, nil
}

func (c *Client) SaveSkill(name, prompt string, description *string, createdBy *string) error {
	body := map[string]interface{}{
		"prompt":      prompt,
		"description": description,
		"created_by":  createdBy,
	}
	b, _ := json.Marshal(body)
	resp, err := c.httpClient.Post(c.baseURL+"/skills/"+name, "application/json", bytes.NewReader(b))
	if err != nil {
		return err
	}
	resp.Body.Close()
	return nil
}

func (c *Client) DeleteSkill(name string) error {
	req, err := http.NewRequest(http.MethodDelete, c.baseURL+"/skills/"+name, nil)
	if err != nil {
		return err
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	resp.Body.Close()
	return nil
}

func (c *Client) LoadNotes(userID string) (map[string]string, error) {
	resp, err := c.httpClient.Get(c.baseURL + "/notes/" + userID)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	var v struct {
		Notes map[string]string `json:"notes"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&v); err != nil {
		return nil, err
	}
	return v.Notes, nil
}

func (c *Client) SaveNote(userID, name, content string) error {
	body := map[string]string{"content": content}
	b, _ := json.Marshal(body)
	resp, err := c.httpClient.Post(
		fmt.Sprintf("%s/notes/%s/%s", c.baseURL, userID, name),
		"application/json", bytes.NewReader(b))
	if err != nil {
		return err
	}
	resp.Body.Close()
	return nil
}

func (c *Client) DeleteNote(userID, name string) error {
	req, err := http.NewRequest(http.MethodDelete,
		fmt.Sprintf("%s/notes/%s/%s", c.baseURL, userID, name), nil)
	if err != nil {
		return err
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	resp.Body.Close()
	return nil
}

type Memory struct {
	Content string `json:"content"`
}

func (c *Client) LoadMemory(userID string) (string, error) {
	resp, err := c.httpClient.Get(c.baseURL + "/memory/" + userID)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	var v struct {
		Content string `json:"content"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&v); err != nil {
		return "", err
	}
	return v.Content, nil
}
